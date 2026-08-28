use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AgentKind, BackupEntry, BackupManifest, BackupRecord, CleanerError, CleanupPlan, FileIdentity,
    PathPolicy, atomic_replace_file,
    platform::{atomic_commit_new_file, sync_committed_file, sync_parent_directory},
    safe_remove,
    safety::is_redirecting_path,
    validate_new_file_destination,
};

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

#[derive(Debug)]
struct RestoreTarget {
    root: PathBuf,
    staged: PathBuf,
    destination: PathBuf,
    sha256: String,
    size_bytes: u64,
    partial: Option<(PathBuf, FileIdentity)>,
    committed_identity: Option<FileIdentity>,
}

#[derive(Debug)]
struct CreatedRestoreDirectory {
    root: PathBuf,
    path: PathBuf,
    identity: FileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreEvent {
    BeforeCommit(usize),
    AfterCommit(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupEvent {
    BeforeArchiveCreation(Uuid),
    AfterArchiveCreation(Uuid),
    BeforeArchiveVerification(Uuid),
    AfterArchiveVerification(Uuid),
    BeforeCatalogCommit(Uuid),
    AfterCatalogCommit(Uuid),
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
        self.create_backup_with_hook(plan, agent, agent_version, sources, |_| Ok(()))
    }

    pub fn create_backup_with_hook<F>(
        &self,
        plan: &CleanupPlan,
        agent: AgentKind,
        agent_version: Option<String>,
        sources: &[BackupSource],
        mut hook: F,
    ) -> Result<BackupManifest, CleanerError>
    where
        F: FnMut(BackupEvent) -> Result<(), CleanerError>,
    {
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

        hook(BackupEvent::BeforeArchiveCreation(backup_id))?;
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial_path)?;
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
        atomic_replace_file(&partial_path, &archive_path)?;
        hook(BackupEvent::AfterArchiveCreation(backup_id))?;
        hook(BackupEvent::BeforeArchiveVerification(backup_id))?;
        if let Err(error) = verify_committed_archive(&archive_path, &identity, &manifest) {
            let _ = fs::remove_file(&archive_path);
            return Err(error);
        }
        hook(BackupEvent::AfterArchiveVerification(backup_id))?;

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
        hook(BackupEvent::BeforeCatalogCommit(backup_id))?;
        self.save_catalog(&catalog)?;
        hook(BackupEvent::AfterCatalogCommit(backup_id))?;
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
        self.restore_with_hook(backup_id, expected_agent, roots, |_| Ok(()))
    }

    fn restore_with_hook<F>(
        &self,
        backup_id: Uuid,
        expected_agent: AgentKind,
        roots: &BTreeMap<String, PathBuf>,
        mut hook: F,
    ) -> Result<BackupManifest, CleanerError>
    where
        F: FnMut(RestoreEvent) -> Result<(), CleanerError>,
    {
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
        let staging_parent = self.base_dir.join("restore-staging");
        fs::create_dir_all(&staging_parent)?;
        restrict_directory(&staging_parent)?;
        let staging = tempfile::Builder::new()
            .prefix("restore-")
            .tempdir_in(&staging_parent)?;
        restrict_directory(staging.path())?;

        let mut manifest: Option<BackupManifest> = None;
        let mut extracted_payloads = HashSet::<PathBuf>::new();
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
                if manifest.is_some() {
                    return Err(CleanerError::Backup(
                        "backup contains multiple manifests".into(),
                    ));
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                manifest = Some(serde_json::from_slice(&bytes)?);
                continue;
            }
            let payload_path = path
                .strip_prefix("payload")
                .map_err(|_| CleanerError::Backup("unexpected archive path".into()))?;
            validate_relative_path(payload_path)?;
            if payload_path.components().count() < 2
                || !extracted_payloads.insert(payload_path.to_path_buf())
            {
                return Err(CleanerError::Backup(format!(
                    "invalid or duplicate backup payload: {}",
                    payload_path.display()
                )));
            }
            let target = staging.path().join(payload_path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            restrict_file(&target)?;
            io::copy(&mut entry, &mut output)?;
            output.sync_all()?;
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

        let mut expected_payloads = HashSet::<PathBuf>::new();
        let mut destinations = HashSet::<PathBuf>::new();
        let mut targets = Vec::<RestoreTarget>::with_capacity(manifest.entries.len());
        for expected in &manifest.entries {
            validate_root_label(&expected.root)?;
            let root = roots.get(&expected.root).ok_or_else(|| {
                CleanerError::Backup(format!("missing restore root: {}", expected.root))
            })?;
            let relative = portable_to_path(&expected.relative_path)?;
            let payload_path = PathBuf::from(&expected.root).join(&relative);
            if !expected_payloads.insert(payload_path.clone()) {
                return Err(CleanerError::Backup(format!(
                    "duplicate manifest destination: {}/{}",
                    expected.root, expected.relative_path
                )));
            }
            let staged = staging.path().join(&payload_path);
            let staged_metadata = fs::symlink_metadata(&staged)?;
            if !staged_metadata.is_file()
                || is_redirecting_path(&staged)?
                || staged_metadata.len() != expected.size_bytes
                || hash_file(&staged)? != expected.sha256
            {
                return Err(CleanerError::Backup(format!(
                    "checksum mismatch for {}",
                    expected.relative_path
                )));
            }
            let destination = validate_new_file_destination(root, &root.join(&relative))?;
            if !destinations.insert(destination.clone()) {
                return Err(CleanerError::Backup(format!(
                    "multiple manifest entries resolve to {}",
                    destination.display()
                )));
            }
            targets.push(RestoreTarget {
                root: root.canonicalize()?,
                staged,
                destination,
                sha256: expected.sha256.clone(),
                size_bytes: expected.size_bytes,
                partial: None,
                committed_identity: None,
            });
        }
        if expected_payloads != extracted_payloads {
            return Err(CleanerError::Backup(
                "backup payload set does not match the manifest".into(),
            ));
        }

        let mut created_directories = Vec::<CreatedRestoreDirectory>::new();
        let transaction = (|| {
            for target in &mut targets {
                prepare_restore_parent(target, &mut created_directories)?;
                target.partial = Some(stage_sibling_file(target)?);
            }

            for target in &targets {
                validate_new_file_destination(&target.root, &target.destination)?;
            }

            for (index, target) in targets.iter_mut().enumerate() {
                hook(RestoreEvent::BeforeCommit(index))?;
                let partial = target
                    .partial
                    .as_ref()
                    .expect("restore payloads were staged")
                    .0
                    .clone();
                let staged_identity = target
                    .partial
                    .as_ref()
                    .expect("restore payloads were staged")
                    .1
                    .clone();
                atomic_commit_new_file(&partial, &target.destination)?;
                target.committed_identity = Some(staged_identity.clone());
                let committed_identity = FileIdentity::capture(&target.destination)?;
                if !staged_identity.same_object(&committed_identity) {
                    return Err(CleanerError::UnsafePath(format!(
                        "restored file identity changed during commit: {}",
                        target.destination.display()
                    )));
                }
                target.committed_identity = Some(committed_identity);
                hook(RestoreEvent::AfterCommit(index))?;
                sync_committed_file(&target.destination)?;
            }

            for target in &targets {
                let metadata = fs::symlink_metadata(&target.destination)?;
                if !metadata.is_file()
                    || is_redirecting_path(&target.destination)?
                    || metadata.len() != target.size_bytes
                    || hash_file(&target.destination)? != target.sha256
                {
                    return Err(CleanerError::Backup(format!(
                        "restored file verification failed: {}",
                        target.destination.display()
                    )));
                }
            }
            Ok(())
        })();

        if let Err(error) = transaction {
            return match rollback_restore(&mut targets, &mut created_directories) {
                Ok(()) => Err(error),
                Err(rollback) => Err(CleanerError::Backup(format!(
                    "{error}; restore rollback also failed: {rollback}"
                ))),
            };
        }

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
        let partial = self
            .base_dir
            .join(format!(".{CATALOG_FILE}.{}.partial", Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        restrict_file(&partial)?;
        if let Err(error) = serde_json::to_writer_pretty(file, catalog) {
            let _ = fs::remove_file(&partial);
            return Err(error.into());
        }
        if let Err(error) = atomic_replace_file(&partial, &path) {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        Ok(())
    }
}

fn prepare_restore_parent(
    target: &RestoreTarget,
    created: &mut Vec<CreatedRestoreDirectory>,
) -> Result<(), CleanerError> {
    validate_new_file_destination(&target.root, &target.destination)?;
    let parent = target
        .destination
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(target.destination.display().to_string()))?;
    let relative_parent = parent
        .strip_prefix(&target.root)
        .map_err(|_| CleanerError::UnsafePath(parent.display().to_string()))?;
    let mut current = target.root.clone();
    for component in relative_parent.components() {
        let Component::Normal(part) = component else {
            return Err(CleanerError::UnsafePath(parent.display().to_string()));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                let identity = match FileIdentity::capture(&current) {
                    Ok(identity) => identity,
                    Err(error) => {
                        let _ = fs::remove_dir(&current);
                        return Err(error);
                    }
                };
                created.push(CreatedRestoreDirectory {
                    root: target.root.clone(),
                    path: current.clone(),
                    identity,
                });
                restrict_directory(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
        validate_new_file_destination(&target.root, &target.destination)?;
    }
    Ok(())
}

fn stage_sibling_file(target: &RestoreTarget) -> Result<(PathBuf, FileIdentity), CleanerError> {
    let parent = target
        .destination
        .parent()
        .ok_or_else(|| CleanerError::UnsafePath(target.destination.display().to_string()))?;
    let partial = parent.join(format!(".cleanerx-restore-{}.partial", Uuid::new_v4()));
    validate_new_file_destination(&target.root, &partial)?;

    let result = (|| {
        let mut input = File::open(&target.staged)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        restrict_file(&partial)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);

        let metadata = fs::symlink_metadata(&partial)?;
        if !metadata.is_file()
            || is_redirecting_path(&partial)?
            || metadata.len() != target.size_bytes
            || hash_file(&partial)? != target.sha256
        {
            return Err(CleanerError::Backup(format!(
                "staged restore verification failed: {}",
                target.destination.display()
            )));
        }
        FileIdentity::capture(&partial)
    })();

    match result {
        Ok(identity) => Ok((partial, identity)),
        Err(error) => {
            let _ = remove_plain_file_if_present(&partial);
            Err(error)
        }
    }
}

fn rollback_restore(
    targets: &mut [RestoreTarget],
    created_directories: &mut [CreatedRestoreDirectory],
) -> Result<(), CleanerError> {
    let mut failures = Vec::<String>::new();

    for target in targets.iter_mut().rev() {
        let Some(identity) = target.committed_identity.take() else {
            continue;
        };
        let policy = PathPolicy::new(vec![target.root.clone()], vec![]);
        match safe_remove(&target.destination, &policy, Some(&identity))
            .and_then(|_| sync_parent_directory(&target.destination))
        {
            Ok(()) => {}
            Err(error) => failures.push(format!("{}: {error}", target.destination.display())),
        }
    }

    for target in targets.iter_mut().rev() {
        let Some((partial, identity)) = target.partial.take() else {
            continue;
        };
        match fs::symlink_metadata(&partial) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{}: {error}", partial.display())),
            Ok(_) => {
                let policy = PathPolicy::new(vec![target.root.clone()], vec![]);
                match safe_remove(&partial, &policy, Some(&identity))
                    .and_then(|_| sync_parent_directory(&partial))
                {
                    Ok(()) => {}
                    Err(error) => failures.push(format!("{}: {error}", partial.display())),
                }
            }
        }
    }

    for directory in created_directories.iter_mut().rev() {
        let policy = PathPolicy::new(vec![directory.root.clone()], vec![]);
        match policy.validate_existing(&directory.path).and_then(|path| {
            let current = FileIdentity::capture(&path)?;
            if !directory.identity.same_object(&current) {
                return Err(CleanerError::UnsafePath(format!(
                    "restore directory changed before rollback: {}",
                    path.display()
                )));
            }
            fs::remove_dir(&path)?;
            sync_parent_directory(&path)
        }) {
            Ok(()) => {}
            Err(error) => failures.push(format!("{}: {error}", directory.path.display())),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(CleanerError::Backup(failures.join("; ")))
    }
}

fn remove_plain_file_if_present(path: &Path) -> Result<(), CleanerError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.is_file() && !is_redirecting_path(path)? => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(CleanerError::UnsafePath(path.display().to_string())),
    }
}

fn verify_committed_archive(
    archive_path: &Path,
    identity: &age::x25519::Identity,
    expected_manifest: &BackupManifest,
) -> Result<(), CleanerError> {
    let input = BufReader::new(File::open(archive_path)?);
    let decryptor = age::Decryptor::new_buffered(input)
        .map_err(|error| CleanerError::Backup(error.to_string()))?;
    let reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|error| CleanerError::Backup(error.to_string()))?;
    let decoder = zstd::Decoder::new(reader)?;
    let mut archive = tar::Archive::new(decoder);
    let mut manifest = None;
    let mut payload = BTreeMap::<(String, String), (String, u64)>::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(CleanerError::Backup(
                "backup verification found a non-file entry".into(),
            ));
        }
        let path = entry.path()?.into_owned();
        validate_relative_path(&path)?;
        if path == Path::new("manifest.json") {
            if manifest.is_some() {
                return Err(CleanerError::Backup(
                    "backup contains duplicate manifests".into(),
                ));
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            manifest = Some(serde_json::from_slice::<BackupManifest>(&bytes)?);
            continue;
        }

        let payload_path = path
            .strip_prefix("payload")
            .map_err(|_| CleanerError::Backup("unexpected archive path".into()))?;
        let mut components = payload_path.components();
        let root = match components.next() {
            Some(Component::Normal(root)) => root.to_string_lossy().into_owned(),
            _ => return Err(CleanerError::Backup("missing payload root".into())),
        };
        validate_root_label(&root)?;
        let relative = components.collect::<PathBuf>();
        validate_relative_path(&relative)?;
        if relative.as_os_str().is_empty() {
            return Err(CleanerError::Backup("missing payload path".into()));
        }
        let portable = path_to_portable(&relative)?;
        let (sha256, size_bytes) = hash_reader(&mut entry)?;
        if payload
            .insert((root, portable), (sha256, size_bytes))
            .is_some()
        {
            return Err(CleanerError::Backup(
                "backup contains duplicate payload entries".into(),
            ));
        }
    }

    let manifest = manifest.ok_or_else(|| CleanerError::Backup("missing manifest".into()))?;
    if serde_json::to_value(&manifest)? != serde_json::to_value(expected_manifest)? {
        return Err(CleanerError::Backup(
            "committed backup manifest did not match the planned archive".into(),
        ));
    }
    let expected_payload = expected_manifest
        .entries
        .iter()
        .map(|entry| {
            (
                (entry.root.clone(), entry.relative_path.clone()),
                (entry.sha256.clone(), entry.size_bytes),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if payload != expected_payload {
        return Err(CleanerError::Backup(
            "committed backup payload failed hash verification".into(),
        ));
    }
    Ok(())
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
        if is_redirecting_path(&source.root_path)? || is_redirecting_path(&source.path)? {
            return Err(CleanerError::UnsafePath(source.path.display().to_string()));
        }
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
                if is_redirecting_path(entry.path())? {
                    return Err(CleanerError::UnsafePath(entry.path().display().to_string()));
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
    if is_redirecting_path(path)? {
        return Err(CleanerError::UnsafePath(path.display().to_string()));
    }
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
    hash_reader(&mut file).map(|(sha256, _)| sha256)
}

fn hash_reader(reader: &mut impl Read) -> Result<(String, u64), CleanerError> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size.saturating_add(count as u64);
        digest.update(&buffer[..count]);
    }
    Ok((hex::encode(digest.finalize()), size))
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

    #[test]
    fn backup_fault_injection_covers_every_commit_and_verification_boundary() {
        for fault_index in 0..6 {
            let workspace = tempfile::tempdir().expect("workspace");
            let backup_dir = tempfile::tempdir().expect("backups");
            let source = workspace.path().join("sessions/thread.jsonl");
            fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
            fs::write(&source, b"protected source bytes").expect("source");
            let identity = age::x25519::Identity::generate();
            let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
                .expect("store");
            let plan = CleanupPlan {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                created_at: Utc::now(),
                selected_item_ids: vec!["session:test".into()],
                expanded_session_ids: vec!["test".into()],
                operations: Vec::new(),
                estimated_bytes: 22,
                estimated_backup_bytes: 22,
                blockers: Vec::new(),
            };
            let mut event_index = 0;
            let result = store.create_backup_with_hook(
                &plan,
                AgentKind::Codex,
                Some("test".into()),
                &[BackupSource {
                    root_label: "codex_home".into(),
                    root_path: workspace.path().to_path_buf(),
                    path: source.clone(),
                }],
                |_| {
                    let current = event_index;
                    event_index += 1;
                    if current == fault_index {
                        Err(CleanerError::Backup(format!(
                            "injected backup fault at boundary {current}"
                        )))
                    } else {
                        Ok(())
                    }
                },
            );

            assert!(matches!(result, Err(CleanerError::Backup(_))));
            assert_eq!(
                fs::read(&source).expect("source bytes"),
                b"protected source bytes"
            );
            assert!(
                fs::read_dir(backup_dir.path())
                    .expect("backup directory")
                    .all(|entry| !entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".partial"))
            );
            let catalog = store.list().expect("catalog remains readable");
            if fault_index == 5 {
                assert_eq!(
                    catalog.len(),
                    1,
                    "catalog commit happened before final hook"
                );
            } else {
                assert!(catalog.is_empty(), "uncommitted backup entered the catalog");
            }
        }
    }

    #[test]
    fn restore_rolls_back_every_commit_boundary() {
        let events = [
            RestoreEvent::BeforeCommit(0),
            RestoreEvent::AfterCommit(0),
            RestoreEvent::BeforeCommit(1),
            RestoreEvent::AfterCommit(1),
        ];

        for injected in events {
            let workspace = tempfile::tempdir().expect("workspace");
            let source_tree = tempfile::tempdir().expect("source tree");
            let protected_source = source_tree.path().join("source.rs");
            fs::write(&protected_source, b"fn protected() {}").expect("protected source");
            let sessions = workspace.path().join("sessions/nested");
            fs::create_dir_all(&sessions).expect("session directory");
            let first = sessions.join("first.jsonl");
            let second = sessions.join("second.jsonl");
            fs::write(&first, b"first private transcript").expect("first transcript");
            fs::write(&second, b"second private transcript").expect("second transcript");

            let backup_dir = tempfile::tempdir().expect("backups");
            let identity = age::x25519::Identity::generate();
            let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
                .expect("store");
            let plan = CleanupPlan {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                created_at: Utc::now(),
                selected_item_ids: vec!["session:first".into(), "session:second".into()],
                expanded_session_ids: vec!["first".into(), "second".into()],
                operations: Vec::<PlannedOperation>::new(),
                estimated_bytes: 48,
                estimated_backup_bytes: 48,
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
                        path: sessions.clone(),
                    }],
                )
                .expect("backup");
            fs::remove_dir_all(workspace.path().join("sessions")).expect("remove originals");
            let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);

            let result = store.restore_with_hook(manifest.id, AgentKind::Codex, &roots, |event| {
                if event == injected {
                    Err(CleanerError::Backup(format!(
                        "injected restore fault at {event:?}"
                    )))
                } else {
                    Ok(())
                }
            });

            assert!(matches!(result, Err(CleanerError::Backup(_))));
            assert!(
                !workspace.path().join("sessions").exists(),
                "destination tree changed after {injected:?}"
            );
            assert_eq!(
                fs::read(&protected_source).expect("protected source bytes"),
                b"fn protected() {}"
            );
        }
    }

    #[test]
    fn restore_preflight_never_overwrites_an_existing_destination() {
        let workspace = tempfile::tempdir().expect("workspace");
        let backup_dir = tempfile::tempdir().expect("backups");
        let first = workspace.path().join("sessions/first.jsonl");
        let second = workspace.path().join("sessions/second.jsonl");
        fs::create_dir_all(first.parent().expect("parent")).expect("session directory");
        fs::write(&first, b"first backup bytes").expect("first transcript");
        fs::write(&second, b"second backup bytes").expect("second transcript");
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
            estimated_bytes: 36,
            estimated_backup_bytes: 36,
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
                    path: workspace.path().join("sessions"),
                }],
            )
            .expect("backup");
        fs::remove_file(&first).expect("remove first");
        fs::write(&second, b"new writer bytes").expect("replace second");
        let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);

        assert!(matches!(
            store.restore(manifest.id, AgentKind::Codex, &roots),
            Err(CleanerError::Blocked(_))
        ));
        assert!(!first.exists());
        assert_eq!(fs::read(second).expect("writer bytes"), b"new writer bytes");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn restore_rejects_a_redirecting_destination_parent() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let backup_dir = tempfile::tempdir().expect("backups");
        let session = workspace.path().join("sessions/thread.jsonl");
        fs::create_dir_all(session.parent().expect("parent")).expect("session directory");
        fs::write(&session, b"private transcript").expect("transcript");
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
                    path: session.clone(),
                }],
            )
            .expect("backup");
        fs::remove_file(&session).expect("remove original");
        fs::remove_dir(workspace.path().join("sessions")).expect("remove session directory");
        let redirect = workspace.path().join("sessions");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &redirect).expect("symlink");
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&redirect)
                .arg(outside.path())
                .output()
                .expect("create junction");
            assert!(
                output.status.success(),
                "mklink failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);

        assert!(matches!(
            store.restore(manifest.id, AgentKind::Codex, &roots),
            Err(CleanerError::UnsafePath(_))
        ));
        assert!(!outside.path().join("thread.jsonl").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn backup_rejects_redirecting_entries_and_preserves_external_bytes() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("private"), b"private").expect("outside bytes");
        let redirect = workspace.path().join("sessions-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &redirect).expect("symlink");
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&redirect)
                .arg(outside.path())
                .output()
                .expect("create junction");
            assert!(
                output.status.success(),
                "mklink failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let backup_dir = tempfile::tempdir().expect("backups");
        let identity = age::x25519::Identity::generate();
        let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
            .expect("store");
        let plan = CleanupPlan {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            created_at: Utc::now(),
            selected_item_ids: vec!["session:redirect".into()],
            expanded_session_ids: vec!["redirect".into()],
            operations: Vec::<PlannedOperation>::new(),
            estimated_bytes: 7,
            estimated_backup_bytes: 7,
            blockers: vec![],
        };
        let result = store.create_backup(
            &plan,
            AgentKind::Codex,
            Some("test".into()),
            &[BackupSource {
                root_label: "codex_home".into(),
                root_path: workspace.path().to_path_buf(),
                path: workspace.path().to_path_buf(),
            }],
        );

        assert!(matches!(result, Err(CleanerError::UnsafePath(_))));
        assert_eq!(
            fs::read(outside.path().join("private")).expect("outside bytes"),
            b"private"
        );
        assert!(store.list().expect("empty catalog").is_empty());
    }

    #[test]
    fn repeated_catalog_updates_replace_the_previous_file() {
        let backup_dir = tempfile::tempdir().expect("backups");
        let identity = age::x25519::Identity::generate();
        let store = BackupStore::with_identity(backup_dir.path().to_path_buf(), 30, &identity)
            .expect("store");
        store
            .save_catalog(&BackupCatalog::default())
            .expect("first catalog");
        store
            .save_catalog(&BackupCatalog::default())
            .expect("replacement catalog");
        assert!(backup_dir.path().join(CATALOG_FILE).is_file());
        assert!(
            fs::read_dir(backup_dir.path())
                .expect("catalog directory")
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        );
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

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an isolated Windows Credential Manager account"]
    fn live_windows_credential_manager_backup_round_trip() {
        assert_eq!(
            std::env::var("CLEANERX_WINDOWS_CREDENTIAL_MANAGER_TEST").as_deref(),
            Ok("1"),
            "run only inside the isolated Windows CI account"
        );
        let credential = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .expect("Windows Credential Manager backend");
        let _ = credential.delete_credential();

        let workspace = tempfile::tempdir().expect("workspace");
        let backup_dir = tempfile::tempdir().expect("backups");
        let source = workspace.path().join("sessions/thread.jsonl");
        fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
        fs::write(&source, b"private Windows transcript").expect("write");
        let store = BackupStore::new(backup_dir.path().to_path_buf(), 30).expect("store");
        let plan = CleanupPlan {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            created_at: Utc::now(),
            selected_item_ids: vec!["session:windows-test".into()],
            expanded_session_ids: vec!["windows-test".into()],
            operations: Vec::<PlannedOperation>::new(),
            estimated_bytes: 26,
            estimated_backup_bytes: 26,
            blockers: vec![],
        };
        let manifest = store
            .create_backup(
                &plan,
                AgentKind::Codex,
                Some("windows-test".into()),
                &[BackupSource {
                    root_label: "codex_home".into(),
                    root_path: workspace.path().to_path_buf(),
                    path: source.clone(),
                }],
            )
            .expect("Credential Manager-backed backup");
        fs::remove_file(&source).expect("remove original");
        let roots = BTreeMap::from([("codex_home".into(), workspace.path().to_path_buf())]);
        store
            .restore(manifest.id, AgentKind::Codex, &roots)
            .expect("Credential Manager-backed restore");

        assert_eq!(
            fs::read(source).expect("restored bytes"),
            b"private Windows transcript"
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
