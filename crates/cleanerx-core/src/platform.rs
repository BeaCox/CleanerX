use std::fs;
#[cfg(not(windows))]
use std::fs::File;
use std::path::Path;

use crate::CleanerError;

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
