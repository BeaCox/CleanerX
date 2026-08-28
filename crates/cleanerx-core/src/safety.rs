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
    pub metadata_revision: String,
}

impl FileIdentity {
    pub fn capture(path: &Path) -> Result<Self, CleanerError> {
        let metadata = fs::symlink_metadata(path)?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

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
        reject_lexical_escape(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(CleanerError::UnsafePath(format!(
                "symbolic links are never mutation targets: {}",
                path.display()
            )));
        }

        let canonical = path.canonicalize()?;
        let allowed_root = self
            .allowed_roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .filter(|allowed| canonical.starts_with(allowed))
            .max_by_key(|allowed| allowed.components().count())
            .ok_or_else(|| {
                CleanerError::UnsafePath(format!("outside allowed roots: {}", path.display()))
            })?;

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
    if metadata.file_type().is_symlink() {
        return Err(CleanerError::UnsafePath(format!(
            "symbolic link inside mutation target: {}",
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

fn validate_tree(path: &Path, allowed_root: &Path) -> Result<(), CleanerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let root_metadata = fs::symlink_metadata(allowed_root)?;
        validate_tree_on_device(path, root_metadata.dev())
    }
    #[cfg(not(unix))]
    {
        validate_tree_on_device(path, 0)
    }
}

fn validate_tree_on_device(path: &Path, expected_device: u64) -> Result<(), CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(CleanerError::UnsafePath(format!(
            "symbolic links are never mutation targets: {}",
            path.display()
        )));
    }
    validate_owner(path, &metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        validate_device_boundary(path, expected_device, metadata.dev())?;
    }
    #[cfg(not(unix))]
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

#[cfg(not(unix))]
fn validate_owner(_path: &Path, _metadata: &fs::Metadata) -> Result<(), CleanerError> {
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
    if metadata.file_type().is_symlink() || metadata.is_file() {
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
    if metadata.file_type().is_symlink() {
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

    #[test]
    fn rejects_symlink_escape() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let link = root.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let policy = PathPolicy::new(vec![root.path().to_path_buf()], vec![]);
        #[cfg(unix)]
        assert!(policy.validate_existing(&link).is_err());
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
