use std::fs;
#[cfg(not(windows))]
use std::fs::File;
use std::path::Path;
use std::process::Command;

use crate::CleanerError;

/// Configures a child process that is driven entirely through pipes by the desktop app.
///
/// Windows GUI applications otherwise create a visible console for CLI launchers such as
/// `codex.cmd` and `claude.exe`. The flag changes only window creation; stdio and exit status keep
/// their normal `Command` semantics.
pub fn configure_background_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        command.creation_flags(background_command_creation_flags());
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(windows)]
const fn background_command_creation_flags() -> u32 {
    windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
}

/// Durably commits a fully written sibling file over `destination`.
///
/// Callers must close their writer before invoking this function. The source is flushed first,
/// then committed with replacement semantics that are atomic on the supported local filesystems.
pub fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), CleanerError> {
    validate_atomic_paths(source, destination)?;
    sync_file(source)?;
    replace_file(source, destination)?;
    sync_file(destination)?;
    sync_parent(destination)?;
    Ok(())
}

/// Atomically moves a fully written sibling file into a destination that must not exist.
///
/// On success the destination has been created and the source no longer exists. On failure the
/// destination was not created. Callers that require a durable multi-file transaction must record
/// the successful commit before calling [`sync_committed_file`], so a later flush failure can roll
/// the destination back.
pub(crate) fn atomic_commit_new_file(
    source: &Path,
    destination: &Path,
) -> Result<(), CleanerError> {
    validate_new_file_paths(source, destination)?;
    sync_file(source)?;
    commit_new_file(source, destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            CleanerError::Blocked(format!(
                "restore target already exists: {}",
                destination.display()
            ))
        } else {
            error.into()
        }
    })
}

/// Flushes a newly committed file and its containing directory where the platform supports it.
pub(crate) fn sync_committed_file(path: &Path) -> Result<(), CleanerError> {
    sync_file(path)?;
    sync_parent(path)
}

/// Flushes the directory containing a path after a rollback removal.
pub(crate) fn sync_parent_directory(path: &Path) -> Result<(), CleanerError> {
    sync_parent(path)
}

#[cfg(windows)]
fn sync_file(path: &Path) -> Result<(), CleanerError> {
    // FlushFileBuffers requires a handle with write access on Windows.
    fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(windows))]
fn sync_file(path: &Path) -> Result<(), CleanerError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn validate_atomic_paths(source: &Path, destination: &Path) -> Result<(), CleanerError> {
    if !source.is_absolute() || !destination.is_absolute() {
        return Err(CleanerError::UnsafePath(
            "atomic replacement paths must be absolute".into(),
        ));
    }
    let source_parent = source
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(source.display().to_string()))?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(destination.display().to_string()))?;
    if source_parent.canonicalize()? != destination_parent.canonicalize()? {
        return Err(CleanerError::UnsafePath(
            "atomic replacement requires sibling paths".into(),
        ));
    }
    validate_plain_file(source)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => validate_plain_file(destination)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_new_file_paths(source: &Path, destination: &Path) -> Result<(), CleanerError> {
    if !source.is_absolute() || !destination.is_absolute() {
        return Err(CleanerError::UnsafePath(
            "restore commit paths must be absolute".into(),
        ));
    }
    let source_parent = source
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(source.display().to_string()))?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(destination.display().to_string()))?;
    if source_parent.canonicalize()? != destination_parent.canonicalize()? {
        return Err(CleanerError::UnsafePath(
            "restore commit requires sibling paths".into(),
        ));
    }
    validate_plain_file(source)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(CleanerError::Blocked(format!(
            "restore target already exists: {}",
            destination.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_plain_file(path: &Path) -> Result<(), CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_windows_reparse(&metadata) {
        return Err(CleanerError::UnsafePath(format!(
            "atomic replacement path is not a plain file: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CleanerError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers for the duration of the call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CleanerError> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn commit_new_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers for the duration of the call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn commit_new_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the duration of the call.
    if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn commit_new_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the duration of the call.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn commit_new_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), CleanerError> {
    let parent = path
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(path.display().to_string()))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), CleanerError> {
    // MOVEFILE_WRITE_THROUGH flushes the Windows rename operation. Opening directories for a
    // separate FlushFileBuffers call is not consistently supported across Windows filesystems.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_commands_use_the_platform_window_policy() {
        #[cfg(windows)]
        assert_eq!(
            background_command_creation_flags(),
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
        );
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        configure_background_command(&mut command);
    }

    #[test]
    fn atomically_replaces_an_existing_file() {
        let directory = tempfile::tempdir().expect("atomic fixture");
        let destination = directory.path().join("settings.json");
        let source = directory.path().join("settings.json.partial");
        fs::write(&destination, b"old").expect("old value");
        fs::write(&source, b"new").expect("new value");

        atomic_replace_file(&source, &destination).expect("atomic replacement");

        assert_eq!(fs::read(&destination).expect("committed value"), b"new");
        assert!(!source.exists());
    }

    #[test]
    fn atomically_commits_a_new_file_without_overwrite() {
        let directory = tempfile::tempdir().expect("atomic fixture");
        let destination = directory.path().join("restored.jsonl");
        let source = directory.path().join("restored.jsonl.partial");
        fs::write(&source, b"restored").expect("staged value");

        atomic_commit_new_file(&source, &destination).expect("new-file commit");
        sync_committed_file(&destination).expect("durable commit");

        assert_eq!(
            fs::read(&destination).expect("committed value"),
            b"restored"
        );
        assert!(!source.exists());

        let second_source = directory.path().join("second.partial");
        fs::write(&second_source, b"replacement").expect("second staged value");
        assert!(matches!(
            atomic_commit_new_file(&second_source, &destination),
            Err(CleanerError::Blocked(_))
        ));
        assert_eq!(
            fs::read(&destination).expect("preserved value"),
            b"restored"
        );
        assert_eq!(
            fs::read(second_source).expect("uncommitted value"),
            b"replacement"
        );
    }

    #[test]
    fn rejects_a_source_outside_the_destination_directory() {
        let source_directory = tempfile::tempdir().expect("source directory");
        let destination_directory = tempfile::tempdir().expect("destination directory");
        let source = source_directory.path().join("value.partial");
        let destination = destination_directory.path().join("value");
        fs::write(&source, b"new").expect("new value");

        assert!(matches!(
            atomic_replace_file(&source, &destination),
            Err(CleanerError::UnsafePath(_))
        ));
        assert_eq!(fs::read(source).expect("source preserved"), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_dangling_destination_symlink() {
        let directory = tempfile::tempdir().expect("atomic fixture");
        let destination = directory.path().join("settings.json");
        let source = directory.path().join("settings.json.partial");
        fs::write(&source, b"new").expect("new value");
        std::os::unix::fs::symlink(directory.path().join("missing"), &destination)
            .expect("destination symlink");

        assert!(matches!(
            atomic_replace_file(&source, &destination),
            Err(CleanerError::UnsafePath(_))
        ));
        assert_eq!(fs::read(source).expect("source preserved"), b"new");
    }
}
