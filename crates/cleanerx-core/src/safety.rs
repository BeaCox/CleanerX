use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::CleanerError;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub len: u64,
    pub modified_nanos: Option<u128>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(windows)]
    pub volume_serial: u32,
    #[cfg(windows)]
    pub file_index: u64,
    pub metadata_revision: String,
}

impl FileIdentity {
    pub fn capture(path: &Path) -> Result<Self, CleanerError> {
        let metadata = fs::symlink_metadata(path)?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(windows)]
        let windows_identity = windows_file_information(path)?;

        Ok(Self {
            len: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos()),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(windows)]
            volume_serial: windows_identity.dwVolumeSerialNumber,
            #[cfg(windows)]
            file_index: u64::from(windows_identity.nFileIndexHigh) << 32
                | u64::from(windows_identity.nFileIndexLow),
            metadata_revision: metadata_revision(&[path.to_path_buf()])?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PathPolicy {
    allowed_roots: Vec<PathBuf>,
    protected: Vec<PathBuf>,
}

impl PathPolicy {
    pub fn new(allowed_roots: Vec<PathBuf>, protected: Vec<PathBuf>) -> Self {
        Self {
            allowed_roots,
            protected,
        }
    }

    pub fn validate_existing(&self, path: &Path) -> Result<PathBuf, CleanerError> {
        let (canonical, allowed_root) = resolve_existing_beneath(path, &self.allowed_roots)?;

        for protected in &self.protected {
            let protected = protected
                .canonicalize()
                .unwrap_or_else(|_| protected.clone());
            if canonical == protected
                || canonical.starts_with(&protected)
                || protected.starts_with(&canonical)
            {
                return Err(CleanerError::UnsafePath(format!(
                    "protected data boundary: {}",
                    path.display()
                )));
            }
        }

        validate_tree(&canonical, &allowed_root)?;

        Ok(canonical)
    }

    pub fn revalidate_identity(
        &self,
        path: &Path,
        expected: &FileIdentity,
    ) -> Result<PathBuf, CleanerError> {
        let canonical = self.validate_existing(path)?;
        let current = FileIdentity::capture(&canonical)?;
        if &current != expected {
            return Err(CleanerError::UnsafePath(format!(
                "target changed after scan: {}",
                path.display()
            )));
        }
        Ok(canonical)
    }
}

/// Resolves one existing path beneath a fixed root without following a symbolic link or Windows
/// reparse point at any component below that root. This is the read-only counterpart to the full
/// mutation policy; it intentionally does not recurse into a directory or validate ownership.
pub fn validate_existing_beneath(
    path: &Path,
    allowed_roots: &[PathBuf],
) -> Result<PathBuf, CleanerError> {
    resolve_existing_beneath(path, allowed_roots).map(|(canonical, _)| canonical)
}

fn resolve_existing_beneath(
    path: &Path,
    allowed_roots: &[PathBuf],
) -> Result<(PathBuf, PathBuf), CleanerError> {
    reject_lexical_escape(path)?;
    let mut selected_root = None::<PathBuf>;
    for root in allowed_roots {
        let raw_match = path.starts_with(root);
        let canonical_root = root.canonicalize().ok();
        let canonical_match = canonical_root
            .as_ref()
            .is_some_and(|canonical| path.starts_with(canonical));
        if !raw_match && !canonical_match {
            continue;
        }
        if is_redirecting_path(root)? {
            return Err(CleanerError::UnsafePath(format!(
                "allowlisted root is a symbolic link or reparse point: {}",
                root.display()
            )));
        }
        let anchor = if raw_match {
            root.clone()
        } else {
            canonical_root.expect("a canonical match requires a canonical root")
        };
        if selected_root
            .as_ref()
            .is_none_or(|selected| anchor.components().count() > selected.components().count())
        {
            selected_root = Some(anchor);
        }
    }
    let lexical_root = selected_root.ok_or_else(|| {
        CleanerError::UnsafePath(format!("outside allowed roots: {}", path.display()))
    })?;
    validate_path_chain(&lexical_root, path)?;
    let canonical = path.canonicalize()?;
    let allowed_root = lexical_root.canonicalize()?;
    if !canonical.starts_with(&allowed_root) {
        return Err(CleanerError::UnsafePath(format!(
            "outside allowed roots after path resolution: {}",
            path.display()
        )));
    }
    Ok((canonical, allowed_root))
}

fn validate_path_chain(root: &Path, path: &Path) -> Result<(), CleanerError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CleanerError::UnsafePath(path.display().to_string()))?;
    let mut current = root.to_path_buf();
    let root_metadata = fs::symlink_metadata(&current)?;
    if is_redirecting_file(&current, &root_metadata)? {
        return Err(CleanerError::UnsafePath(format!(
            "allowlisted root is a symbolic link or reparse point: {}",
            current.display()
        )));
    }
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(CleanerError::UnsafePath(path.display().to_string()));
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current)?;
        if is_redirecting_file(&current, &metadata)? {
            return Err(CleanerError::UnsafePath(format!(
                "symbolic link or reparse point in mutation path: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

/// Produces a stable revision from path names and filesystem identity metadata without reading
/// file contents. It is suitable for detecting normal writer activity while keeping transcript
/// and memory bodies out of inventory snapshots.
pub fn metadata_revision(paths: &[PathBuf]) -> Result<String, CleanerError> {
    let mut paths = paths.to_vec();
    paths.sort();
    let mut hasher = Sha256::new();
    for path in paths {
        if !path.exists() {
            hasher.update(b"missing\0");
            hasher.update(path.to_string_lossy().as_bytes());
            continue;
        }
        hash_metadata_tree(&path, &path, &mut hasher)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn hash_metadata_tree(root: &Path, path: &Path, hasher: &mut Sha256) -> Result<(), CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if is_redirecting_file(path, &metadata)? {
        return Err(CleanerError::UnsafePath(format!(
            "symbolic link or reparse point inside mutation target: {}",
            path.display()
        )));
    }
    validate_owner(path, &metadata)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(metadata.len().to_le_bytes());
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    hasher.update(modified_nanos.to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
        hasher.update(metadata.uid().to_le_bytes());
        hasher.update(metadata.mode().to_le_bytes());
    }
    #[cfg(windows)]
    {
        let information = windows_file_information(path)?;
        hasher.update(information.dwVolumeSerialNumber.to_le_bytes());
        hasher.update(information.nFileIndexHigh.to_le_bytes());
        hasher.update(information.nFileIndexLow.to_le_bytes());
        hasher.update(information.dwFileAttributes.to_le_bytes());
    }
    hasher.update(if metadata.is_dir() {
        b"dir".as_slice()
    } else {
        b"file".as_slice()
    });
    if metadata.is_dir() {
        let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            hash_metadata_tree(root, &child.path(), hasher)?;
        }
    }
    Ok(())
}

fn validate_tree(path: &Path, _allowed_root: &Path) -> Result<(), CleanerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let root_metadata = fs::symlink_metadata(_allowed_root)?;
        validate_tree_on_device(path, root_metadata.dev())
    }
    #[cfg(windows)]
    {
        let root_information = windows_file_information(_allowed_root)?;
        validate_tree_on_device(path, u64::from(root_information.dwVolumeSerialNumber))
    }
    #[cfg(not(any(unix, windows)))]
    {
        validate_tree_on_device(path, 0)
    }
}

fn validate_tree_on_device(path: &Path, expected_device: u64) -> Result<(), CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if is_redirecting_file(path, &metadata)? {
        return Err(CleanerError::UnsafePath(format!(
            "symbolic links and reparse points are never mutation targets: {}",
            path.display()
        )));
    }
    validate_owner(path, &metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        validate_device_boundary(path, expected_device, metadata.dev())?;
    }
    #[cfg(windows)]
    validate_device_boundary(
        path,
        expected_device,
        u64::from(windows_file_information(path)?.dwVolumeSerialNumber),
    )?;
    #[cfg(not(any(unix, windows)))]
    validate_device_boundary(path, expected_device, expected_device)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            validate_tree_on_device(&entry?.path(), expected_device)?;
        }
    }
    Ok(())
}

fn validate_device_boundary(
    path: &Path,
    expected_device: u64,
    actual_device: u64,
) -> Result<(), CleanerError> {
    if actual_device != expected_device {
        return Err(CleanerError::UnsafePath(format!(
            "filesystem mount boundary inside mutation target: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_owner(path: &Path, metadata: &fs::Metadata) -> Result<(), CleanerError> {
    use std::os::unix::fs::MetadataExt;

    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        return Err(CleanerError::UnsafePath(format!(
            "filesystem ownership mismatch: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_owner(path: &Path, _metadata: &fs::Metadata) -> Result<(), CleanerError> {
    validate_windows_owner(path)
}

#[cfg(not(any(unix, windows)))]
fn validate_owner(_path: &Path, _metadata: &fs::Metadata) -> Result<(), CleanerError> {
    Ok(())
}

fn is_redirecting_file(_path: &Path, metadata: &fs::Metadata) -> Result<bool, CleanerError> {
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        Ok(windows_file_information(_path)?.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    Ok(false)
}

pub(crate) fn is_redirecting_path(path: &Path) -> Result<bool, CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    is_redirecting_file(path, &metadata)
}

#[cfg(windows)]
fn windows_file_information(
    path: &Path,
) -> Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION, CleanerError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `path` is a live NUL-terminated UTF-16 buffer and all other arguments are values or
    // null pointers accepted by CreateFileW.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `handle` is valid and `information` points to writable storage of the required type.
    let succeeded = unsafe { GetFileInformationByHandle(handle, &mut information) };
    // SAFETY: `handle` was returned by CreateFileW and has not been closed yet.
    unsafe { CloseHandle(handle) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(information)
}

#[cfg(windows)]
fn validate_windows_owner(path: &Path) -> Result<(), CleanerError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `path_wide` is NUL-terminated; requested output pointers are valid for the call.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32).into());
    }

    let owner_matches = (|| -> Result<bool, CleanerError> {
        let mut token = std::ptr::null_mut();
        // SAFETY: GetCurrentProcess returns the current pseudo-handle and `token` is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        let result = (|| -> Result<bool, CleanerError> {
            let mut required = 0_u32;
            // SAFETY: a zero-length probe with a null output buffer is the documented size query.
            let queried = unsafe {
                GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required)
            };
            // SAFETY: GetLastError reads thread-local error state immediately after the API call.
            if queried != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                return Err(std::io::Error::last_os_error().into());
            }
            let word_size = std::mem::size_of::<usize>();
            let mut buffer = vec![0_usize; (required as usize).div_ceil(word_size)];
            // SAFETY: `buffer` is aligned and has at least `required` writable bytes.
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: GetTokenInformation initialized the aligned buffer as TOKEN_USER.
            let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            // SAFETY: both SIDs are owned by live buffers until this comparison returns.
            Ok(unsafe { EqualSid(owner, token_user.User.Sid) } != 0)
        })();

        // SAFETY: `token` is a live handle returned by OpenProcessToken.
        unsafe { CloseHandle(token) };
        result
    })();

    // SAFETY: the security descriptor was allocated by GetNamedSecurityInfoW.
    unsafe { LocalFree(descriptor.cast()) };
    if !owner_matches? {
        return Err(CleanerError::UnsafePath(format!(
            "filesystem ownership mismatch: {}",
            path.display()
        )));
    }
    Ok(())
}

fn reject_lexical_escape(path: &Path) -> Result<(), CleanerError> {
    if !path.is_absolute() {
        return Err(CleanerError::UnsafePath(format!(
            "mutation target must be absolute: {}",
            path.display()
        )));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(CleanerError::UnsafePath(format!(
            "parent traversal is forbidden: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Removes an allowlisted target without following symbolic links.
pub fn safe_remove(
    path: &Path,
    policy: &PathPolicy,
    expected: Option<&FileIdentity>,
) -> Result<u64, CleanerError> {
    let canonical = if let Some(identity) = expected {
        policy.revalidate_identity(path, identity)?
    } else {
        policy.validate_existing(path)?
    };
    let size = allocated_size(&canonical)?;
    remove_no_follow(&canonical)?;
    Ok(size)
}

fn remove_no_follow(path: &Path) -> Result<(), CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if is_redirecting_file(path, &metadata)? {
        return Err(CleanerError::UnsafePath(format!(
            "symbolic link or reparse point appeared during deletion: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        fs::remove_file(path)?;
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            remove_no_follow(&entry?.path())?;
        }
        fs::remove_dir(path)?;
    }
    Ok(())
}

pub fn allocated_size(path: &Path) -> Result<u64, CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if is_redirecting_file(path, &metadata)? {
        return Ok(0);
    }
    if metadata.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            return Ok(metadata.blocks().saturating_mul(512));
        }
        #[cfg(not(unix))]
        return Ok(metadata.len());
    }
    let mut total = 0_u64;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            total = total.saturating_add(allocated_size(&entry?.path())?);
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);
        assert!(policy.validate_existing(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_path_through_an_internal_symlink() {
        let root = tempfile::tempdir().expect("root");
        let target = root.path().join("target");
        fs::create_dir(&target).expect("target");
        fs::write(target.join("private"), b"private").expect("target bytes");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);

        assert!(policy.validate_existing(&link.join("private")).is_err());
        assert_eq!(
            fs::read(target.join("private")).expect("target bytes"),
            b"private"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_windows_junction_anywhere_inside_a_directory_target() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("private"), b"private").expect("outside bytes");
        let junction = root.path().join("nested-junction");
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .output()
            .expect("create junction");
        assert!(
            output.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);

        assert!(policy.validate_existing(root.path()).is_err());
        assert_eq!(
            fs::read(outside.path().join("private")).expect("outside bytes"),
            b"private"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_file_path_reached_through_an_internal_junction() {
        let root = tempfile::tempdir().expect("root");
        let target = root.path().join("target");
        fs::create_dir(&target).expect("target");
        fs::write(target.join("private"), b"private").expect("target bytes");
        let junction = root.path().join("nested-junction");
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .expect("create junction");
        assert!(
            output.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);

        assert!(policy.validate_existing(&junction.join("private")).is_err());
        assert_eq!(
            fs::read(target.join("private")).expect("target bytes"),
            b"private"
        );
    }

    #[cfg(windows)]
    #[test]
    fn captures_native_windows_volume_and_file_identity() {
        let root = tempfile::tempdir().expect("identity root");
        let file = root.path().join("session.jsonl");
        fs::write(&file, b"private").expect("fixture");

        let identity = FileIdentity::capture(&file).expect("identity");
        assert_ne!(identity.volume_serial, 0);
        assert_ne!(identity.file_index, 0);
    }

    #[test]
    fn protected_child_prevents_parent_deletion() {
        let root = tempfile::tempdir().expect("root");
        let protected = root.path().join("auth.json");
        fs::write(&protected, "secret").expect("write");
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![protected.clone()]);
        assert!(policy.validate_existing(root.path()).is_err());
        assert!(policy.validate_existing(&protected).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_anywhere_inside_a_directory_target() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("private"), b"private").expect("outside bytes");
        std::os::unix::fs::symlink(outside.path(), root.path().join("nested-link"))
            .expect("nested link");
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);

        assert!(policy.validate_existing(root.path()).is_err());
        assert!(outside.path().join("private").exists());
    }

    #[test]
    fn metadata_revision_changes_without_reading_file_contents_into_the_model() {
        let root = tempfile::tempdir().expect("root");
        let file = root.path().join("session.jsonl");
        fs::write(&file, b"one").expect("first bytes");
        let before = metadata_revision(std::slice::from_ref(&file)).expect("first revision");
        fs::write(&file, b"longer replacement").expect("replacement bytes");
        let after = metadata_revision(std::slice::from_ref(&file)).expect("second revision");
        assert_ne!(before, after);
    }

    #[test]
    fn device_boundary_check_is_platform_independent() {
        let path = Path::new("/allowlisted/mounted-data");
        assert!(validate_device_boundary(path, 7, 7).is_ok());
        assert!(validate_device_boundary(path, 7, 8).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_a_native_linux_mount_boundary() {
        use std::os::unix::fs::MetadataExt;

        let root = PathBuf::from("/");
        let root_device = fs::metadata(&root).expect("root metadata").dev();
        let mounted_file = ["/proc/version", "/sys/kernel/uevent_seqnum"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| {
                fs::metadata(path)
                    .map(|metadata| metadata.dev() != root_device)
                    .unwrap_or(false)
            })
            .expect("Linux test runner exposes a mounted virtual filesystem");
        let policy = PathPolicy::new(vec![root], vec![]);

        assert!(policy.validate_existing(&mounted_file).is_err());
    }
}
