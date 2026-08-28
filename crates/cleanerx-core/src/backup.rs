use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AgentKind, BackupEntry, BackupManifest, BackupRecord, CleanerError, CleanupPlan};

const KEYCHAIN_SERVICE: &str = "com.cleanerx.CleanerX";
const KEYCHAIN_ACCOUNT: &str = "backup-x25519-identity";
const CATALOG_FILE: &str = "catalog.json";

#[derive(Debug, Clone)]
pub struct BackupSource {
    pub root_label: String,
    pub root_path: PathBuf,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct BackupCatalog {
    records: Vec<BackupRecord>,
}

#[derive(Debug, Clone)]
pub struct BackupStore {
    base_dir: PathBuf,
    retention_days: u32,
    identity_override: Option<String>,
}

impl BackupStore {
    pub fn new(base_dir: PathBuf, retention_days: u32) -> Result<Self, CleanerError> {
        fs::create_dir_all(&base_dir)?;
        restrict_directory(&base_dir)?;
        Ok(Self {
            base_dir,
            retention_days,
            identity_override: None,
        })
    }

    #[cfg(test)]
    fn with_identity(
        base_dir: PathBuf,
        retention_days: u32,
        identity: &age::x25519::Identity,
    ) -> Result<Self, CleanerError> {
        fs::create_dir_all(&base_dir)?;
        Ok(Self {
            base_dir,
            retention_days,
            identity_override: Some(identity.to_string().expose_secret().to_owned()),
        })
    }

    pub fn create_backup(
        &self,
        plan: &CleanupPlan,
        agent: AgentKind,
        agent_version: Option<String>,
        sources: &[BackupSource],
    ) -> Result<BackupManifest, CleanerError> {
        let identity = self.identity()?;
        let recipient = identity.to_public();
        let backup_id = Uuid::new_v4();
        let archive_path = self.base_dir.join(format!("{backup_id}.cxb"));
        let partial_path = self.base_dir.join(format!("{backup_id}.cxb.partial"));

        let collected = collect_sources(sources)?;
        if collected.is_empty() {
            return Err(CleanerError::Backup(
                "no existing files were available to back up".into(),
            ));
        }

        let entries = collected
            .iter()
            .map(|file| {
                Ok(BackupEntry {
                    root: file.root_label.clone(),
                    relative_path: path_to_portable(&file.relative_path)?,
                    sha256: hash_file(&file.path)?,
                    size_bytes: file.size_bytes,
                })
            })
            .collect::<Result<Vec<_>, CleanerError>>()?;
        let original_bytes = entries.iter().map(|entry| entry.size_bytes).sum();
        let created_at = Utc::now();
        let mut manifest = BackupManifest {
            format_version: 1,
            id: backup_id,
            agent,
            agent_version,
            created_at,
            expires_at: created_at + Duration::days(i64::from(self.retention_days)),
            operation_id: plan.id,
            item_count: plan.selected_item_ids.len(),
            original_bytes,
            archive_bytes: 0,
            entries,
        };

        let output = File::create(&partial_path)?;
        restrict_file(&partial_path)?;
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .map_err(|error| CleanerError::Backup(error.to_string()))?;
        let age_writer = encryptor.wrap_output(output)?;
        let zstd_writer = zstd::Encoder::new(age_writer, 6)?;
        let mut archive = tar::Builder::new(zstd_writer);

        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        append_bytes(&mut archive, "manifest.json", &manifest_bytes)?;
        for file in &collected {
            let archive_name = PathBuf::from("payload")
                .join(&file.root_label)
                .join(&file.relative_path);
            archive.append_path_with_name(&file.path, archive_name)?;
        }

        let zstd_writer = archive.into_inner()?;
        let age_writer = zstd_writer.finish()?;
        age_writer.finish()?;
        fs::rename(&partial_path, &archive_path)?;

        manifest.archive_bytes = fs::metadata(&archive_path)?.len();
        let mut catalog = self.load_catalog()?;
        catalog.records.push(BackupRecord {
            id: manifest.id,
            created_at: manifest.created_at,
            expires_at: manifest.expires_at,
            archive_path: archive_path.to_string_lossy().into_owned(),
            archive_bytes: manifest.archive_bytes,
            original_bytes: manifest.original_bytes,
            item_count: manifest.item_count,
            operation_id: manifest.operation_id,
            agent: manifest.agent,
        });
        self.save_catalog(&catalog)?;
        Ok(manifest)
    }

    pub fn list(&self) -> Result<Vec<BackupRecord>, CleanerError> {
        let mut records = self.load_catalog()?.records;
        records.retain(|record| Path::new(&record.archive_path).is_file());
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at));
        Ok(records)
    }

    pub fn restore(
        &self,
        backup_id: Uuid,
        expected_agent: AgentKind,
        roots: &BTreeMap<String, PathBuf>,
    ) -> Result<BackupManifest, CleanerError> {
        let record = self
            .load_catalog()?
            .records
            .into_iter()
            .find(|record| record.id == backup_id)
            .ok_or_else(|| CleanerError::NotFound(format!("backup {backup_id}")))?;
        let archive_path = PathBuf::from(&record.archive_path);
        self.validate_archive_path(backup_id, &archive_path)?;

        let identity = self.identity()?;
        let input = BufReader::new(File::open(&archive_path)?);
        let decryptor = age::Decryptor::new_buffered(input)
            .map_err(|error| CleanerError::Backup(error.to_string()))?;
        let reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|error| CleanerError::Backup(error.to_string()))?;
        let decoder = zstd::Decoder::new(reader)?;
        let mut archive = tar::Archive::new(decoder);
        let staging = self
            .base_dir
            .join("restore-staging")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&staging)?;
        restrict_directory(&staging)?;

        let mut manifest: Option<BackupManifest> = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_type = entry.header().entry_type();
            if !entry_type.is_file() {
                return Err(CleanerError::Backup(
                    "backup contains a non-file entry".into(),
                ));
            }
            let path = entry.path()?.into_owned();
            validate_relative_path(&path)?;
            if path == Path::new("manifest.json") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                manifest = Some(serde_json::from_slice(&bytes)?);
                continue;
            }
            let payload_path = path
                .strip_prefix("payload")
                .map_err(|_| CleanerError::Backup("unexpected archive path".into()))?;
            let target = staging.join(payload_path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }

        let manifest = manifest.ok_or_else(|| CleanerError::Backup("missing manifest".into()))?;
        if manifest.id != backup_id || manifest.format_version != 1 {
            return Err(CleanerError::Backup(
                "backup identity or format version mismatch".into(),
            ));
        }
        if manifest.agent != expected_agent {
            return Err(CleanerError::Backup(
                "backup Agent does not match the selected restore target".into(),
            ));
        }

        for expected in &manifest.entries {
            let root = roots.get(&expected.root).ok_or_else(|| {
                CleanerError::Backup(format!("missing restore root: {}", expected.root))
            })?;
            let relative = portable_to_path(&expected.relative_path)?;
            let staged = staging.join(&expected.root).join(&relative);
            if !staged.is_file() || hash_file(&staged)? != expected.sha256 {
                return Err(CleanerError::Backup(format!(
                    "checksum mismatch for {}",
                    expected.relative_path
                )));
            }
            let destination = root.join(&relative);
            if destination.exists() {
                return Err(CleanerError::Blocked(format!(
                    "restore target already exists: {}",
                    destination.display()
                )));
            }
        }

        for expected in &manifest.entries {
            let root = roots
                .get(&expected.root)
                .expect("restore roots were preflighted");
            let relative = portable_to_path(&expected.relative_path)?;
            let staged = staging.join(&expected.root).join(&relative);
            let destination = root.join(&relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::rename(&staged, &destination) {
                Ok(()) => {}
                Err(_) => {
                    fs::copy(&staged, &destination)?;
                    fs::remove_file(&staged)?;
                }
            }
        }

        let _ = fs::remove_dir_all(&staging);
        Ok(manifest)
    }

    pub fn purge(&self, backup_id: Uuid) -> Result<(), CleanerError> {
        let mut catalog = self.load_catalog()?;
        let record = catalog
            .records
            .iter()
            .find(|record| record.id == backup_id)
            .cloned()
            .ok_or_else(|| CleanerError::NotFound(format!("backup {backup_id}")))?;
        let path = PathBuf::from(record.archive_path);
        self.validate_archive_path(backup_id, &path)?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        catalog.records.retain(|record| record.id != backup_id);
        self.save_catalog(&catalog)
    }

    fn validate_archive_path(&self, backup_id: Uuid, path: &Path) -> Result<(), CleanerError> {
        let expected = self.base_dir.join(format!("{backup_id}.cxb"));
        if path != expected {
            return Err(CleanerError::UnsafePath(path.display().to_string()));
        }
        if let Ok(metadata) = fs::symlink_metadata(path)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err(CleanerError::UnsafePath(path.display().to_string()));
        }
        Ok(())
    }

    fn identity(&self) -> Result<age::x25519::Identity, CleanerError> {
        if let Some(secret) = &self.identity_override {
            return age::x25519::Identity::from_str(secret)
                .map_err(|error| CleanerError::Backup(error.into()));
        }
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|error| CleanerError::Backup(error.to_string()))?;
        if let Ok(secret) = entry.get_password() {
            return age::x25519::Identity::from_str(&secret)
                .map_err(|error| CleanerError::Backup(error.into()));
        }
        let identity = age::x25519::Identity::generate();
        entry
            .set_password(identity.to_string().expose_secret())
            .map_err(|error| CleanerError::Backup(error.to_string()))?;
        Ok(identity)
    }

    fn load_catalog(&self) -> Result<BackupCatalog, CleanerError> {
        let path = self.base_dir.join(CATALOG_FILE);
        if !path.exists() {
            return Ok(BackupCatalog::default());
        }
        Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
    }

    fn save_catalog(&self, catalog: &BackupCatalog) -> Result<(), CleanerError> {
        let path = self.base_dir.join(CATALOG_FILE);
        let partial = self.base_dir.join(format!("{CATALOG_FILE}.partial"));
        let file = File::create(&partial)?;
        restrict_file(&partial)?;
        serde_json::to_writer_pretty(file, catalog)?;
        fs::rename(partial, path)?;
        Ok(())
    }
}

#[derive(Debug)]
struct CollectedFile {
    root_label: String,
    relative_path: PathBuf,
    path: PathBuf,
    size_bytes: u64,
}

fn collect_sources(sources: &[BackupSource]) -> Result<Vec<CollectedFile>, CleanerError> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for source in sources {
        validate_root_label(&source.root_label)?;
        let root = source.root_path.canonicalize()?;
        let source_path = source.path.canonicalize()?;
        if !source_path.starts_with(&root) {
            return Err(CleanerError::UnsafePath(source.path.display().to_string()));
        }
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(CleanerError::UnsafePath(source.path.display().to_string()));
        }
        if metadata.is_file() {
            push_collected(
                &mut files,
                &mut seen,
                &source.root_label,
                &root,
                &source_path,
            )?;
        } else if metadata.is_dir() {
            for entry in walkdir::WalkDir::new(&source_path).follow_links(false) {
                let entry = entry.map_err(|error| CleanerError::Backup(error.to_string()))?;
                if entry.file_type().is_symlink() {
                    continue;
                }
                if entry.file_type().is_file() {
                    push_collected(
                        &mut files,
                        &mut seen,
                        &source.root_label,
                        &root,
                        entry.path(),
                    )?;
                }
            }
        }
    }
    files.sort_by(|left, right| {
        (&left.root_label, &left.relative_path).cmp(&(&right.root_label, &right.relative_path))
    });
    Ok(files)
}

fn push_collected(
    files: &mut Vec<CollectedFile>,
    seen: &mut HashSet<PathBuf>,
    root_label: &str,
    root: &Path,
    path: &Path,
) -> Result<(), CleanerError> {
    let canonical = path.canonicalize()?;
    if !seen.insert(canonical.clone()) {
        return Ok(());
    }
    let relative_path = canonical
        .strip_prefix(root)
        .map_err(|_| CleanerError::UnsafePath(canonical.display().to_string()))?
        .to_path_buf();
    validate_relative_path(&relative_path)?;
    files.push(CollectedFile {
        root_label: root_label.to_owned(),
        relative_path,
        size_bytes: fs::metadata(&canonical)?.len(),
        path: canonical,
    });
    Ok(())
}

fn validate_root_label(label: &str) -> Result<(), CleanerError> {
    if label.is_empty()
        || !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(CleanerError::Backup(format!(
            "invalid backup root label: {label}"
        )));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), CleanerError> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CleanerError::UnsafePath(path.display().to_string()));
    }
    Ok(())
}

fn path_to_portable(path: &Path) -> Result<String, CleanerError> {
    validate_relative_path(path)?;
    Ok(path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn portable_to_path(portable: &str) -> Result<PathBuf, CleanerError> {
    let path = portable.split('/').filter(|part| !part.is_empty()).fold(
        PathBuf::new(),
        |mut path, part| {
            path.push(part);
            path
        },
    );
    validate_relative_path(&path)?;
    Ok(path)
}

fn append_bytes<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<(), CleanerError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, CleanerError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), CleanerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), CleanerError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), CleanerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), CleanerError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CleanupPlan, PlannedOperation};

    #[test]
    fn encrypted_backup_round_trip() {
        let workspace = tempfile::tempdir().expect("workspace");
        let backup_dir = tempfile::tempdir().expect("backups");
        let source = workspace.path().join("sessions/thread.jsonl");
        fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
        fs::write(&source, b"private transcript").expect("write");
        let identity = age::x25519::Identity::generate();
        let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
            .expect("store");
        let plan = CleanupPlan {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            created_at: Utc::now(),
            selected_item_ids: vec!["session:test".into()],
            expanded_session_ids: vec!["test".into()],
            operations: Vec::<PlannedOperation>::new(),
            estimated_bytes: 18,
            estimated_backup_bytes: 18,
            blockers: vec![],
        };
        let manifest = store
            .create_backup(
                &plan,
                AgentKind::Codex,
                Some("test".into()),
                &[BackupSource {
                    root_label: "codex_home".into(),
                    root_path: workspace.path().to_path_buf(),
                    path: source.clone(),
                }],
            )
            .expect("backup");
        fs::remove_file(&source).expect("remove original");
        let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);
        store
            .restore(manifest.id, AgentKind::Codex, &roots)
            .expect("restore");
        assert_eq!(fs::read(source).expect("read"), b"private transcript");

        let archive_path = PathBuf::from(
            store
                .list()
                .expect("list")
                .first()
                .expect("backup record")
                .archive_path
                .clone(),
        );
        assert!(archive_path.is_file());
        store.purge(manifest.id).expect("purge");
        assert!(!archive_path.exists());
        assert!(store.list().expect("list after purge").is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an isolated Linux Secret Service session"]
    fn live_linux_secret_service_backup_round_trip() {
        assert_eq!(
            std::env::var("CLEANERX_LINUX_SECRET_SERVICE_TEST").as_deref(),
            Ok("1"),
            "run only inside the isolated CI Secret Service session"
        );
        let credential = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .expect("Linux Secret Service backend");
        let _ = credential.delete_credential();

        let workspace = tempfile::tempdir().expect("workspace");
        let backup_dir = tempfile::tempdir().expect("backups");
        let source = workspace.path().join("sessions/thread.jsonl");
        fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
        fs::write(&source, b"private Linux transcript").expect("write");
        let store = BackupStore::new(backup_dir.path().to_path_buf(), 30).expect("store");
        let plan = CleanupPlan {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            created_at: Utc::now(),
            selected_item_ids: vec!["session:linux-test".into()],
            expanded_session_ids: vec!["linux-test".into()],
            operations: Vec::<PlannedOperation>::new(),
            estimated_bytes: 24,
            estimated_backup_bytes: 24,
            blockers: vec![],
        };
        let manifest = store
            .create_backup(
                &plan,
                AgentKind::Codex,
                Some("linux-test".into()),
                &[BackupSource {
                    root_label: "codex_home".into(),
                    root_path: workspace.path().to_path_buf(),
                    path: source.clone(),
                }],
            )
            .expect("Secret Service-backed backup");
        fs::remove_file(&source).expect("remove original");
        let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);
        store
            .restore(manifest.id, AgentKind::Codex, &roots)
            .expect("Secret Service-backed restore");

        assert_eq!(
            fs::read(source).expect("restored bytes"),
            b"private Linux transcript"
        );
        credential
            .delete_credential()
            .expect("remove isolated CI credential");
    }

    #[test]
    fn purge_rejects_a_catalog_path_outside_the_backup_store() {
        let backup_dir = tempfile::tempdir().expect("backups");
        let outside_dir = tempfile::tempdir().expect("outside");
        let outside_archive = outside_dir.path().join("private.cxb");
        fs::write(&outside_archive, b"unrelated bytes").expect("outside archive");
        let identity = age::x25519::Identity::generate();
        let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
            .expect("store");
        let backup_id = Uuid::new_v4();
        store
            .save_catalog(&BackupCatalog {
                records: vec![BackupRecord {
                    id: backup_id,
                    created_at: Utc::now(),
                    expires_at: Utc::now() + Duration::days(30),
                    archive_path: outside_archive.to_string_lossy().into_owned(),
                    archive_bytes: 15,
                    original_bytes: 15,
                    item_count: 1,
                    operation_id: Uuid::new_v4(),
                    agent: AgentKind::Codex,
                }],
            })
            .expect("catalog");

        assert!(matches!(
            store.purge(backup_id),
            Err(CleanerError::UnsafePath(_))
        ));
        assert_eq!(
            fs::read(outside_archive).expect("outside bytes"),
            b"unrelated bytes"
        );
    }
}
