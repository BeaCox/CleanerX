use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::CleanerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub len: u64,
    pub modified_nanos: Option<u128>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
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
        if !self.allowed_roots.iter().any(|root| {
            root.canonicalize()
                .map(|allowed| canonical.starts_with(allowed))
                .unwrap_or(false)
        }) {
            return Err(CleanerError::UnsafePath(format!(
                "outside allowed roots: {}",
                path.display()
            )));
        }

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
}
