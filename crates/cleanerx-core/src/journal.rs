use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::platform::{atomic_replace_file, sync_parent_directory};
use crate::safety::validate_existing_beneath;
use crate::{
    AgentKind, CleanerError, CleanupPlan, OperationKind, OperationStatus, PlannedOperation,
    StorageCategory,
};

pub const JOURNAL_FORMAT_VERSION: u32 = 2;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JournalMutationStatus {
    Pending,
    Running,
    Applied,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JournalBackupStatus {
    NotRequested,
    Pending,
    Writing,
    ArchiveCommitted,
    Verifying,
    Verified,
    CatalogWriting,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalMutation {
    pub index: usize,
    pub kind: OperationKind,
    pub item_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub status: JournalMutationStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub applied_at: Option<DateTime<Utc>>,
}

impl JournalMutation {
    fn from_operation(index: usize, operation: &PlannedOperation) -> Self {
        Self {
            index,
            kind: operation.kind,
            item_ids: operation.item_ids.clone(),
            session_ids: operation.session_ids.clone(),
            status: JournalMutationStatus::Pending,
            started_at: None,
            applied_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationJournal {
    pub format_version: u32,
    pub operation_id: Uuid,
    pub agent: AgentKind,
    pub snapshot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub create_backup: bool,
    pub plan: CleanupPlan,
    pub item_categories: BTreeMap<String, StorageCategory>,
    pub status: OperationStatus,
    pub backup_status: JournalBackupStatus,
    pub backup_candidate_id: Option<Uuid>,
    pub backup_id: Option<Uuid>,
    pub mutations: Vec<JournalMutation>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyOperationJournal {
    operation_id: Uuid,
    status: OperationStatus,
    updated_at: DateTime<Utc>,
    backup_id: Option<Uuid>,
    message: Option<String>,
}

impl OperationJournal {
    pub fn new(plan: CleanupPlan, agent: AgentKind, create_backup: bool) -> Self {
        Self::new_with_item_categories(plan, agent, create_backup, BTreeMap::new())
    }

    pub fn new_with_item_categories(
        plan: CleanupPlan,
        agent: AgentKind,
        create_backup: bool,
        item_categories: BTreeMap<String, StorageCategory>,
    ) -> Self {
        let now = Utc::now();
        let mutations = plan
            .operations
            .iter()
            .enumerate()
            .map(|(index, operation)| JournalMutation::from_operation(index, operation))
            .collect();
        Self {
            format_version: JOURNAL_FORMAT_VERSION,
            operation_id: plan.id,
            agent,
            snapshot_id: plan.snapshot_id,
            created_at: now,
            updated_at: now,
            create_backup,
            plan,
            item_categories,
            status: OperationStatus::Planned,
            backup_status: if create_backup {
                JournalBackupStatus::Pending
            } else {
                JournalBackupStatus::NotRequested
            },
            backup_candidate_id: None,
            backup_id: None,
            mutations,
            message: None,
        }
    }

    pub fn mark_backup_writing(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::Pending,
            JournalBackupStatus::Writing,
            backup_id,
        )
    }

    pub fn mark_backup_archive_committed(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::Writing,
            JournalBackupStatus::ArchiveCommitted,
            backup_id,
        )
    }

    pub fn mark_backup_verifying(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::ArchiveCommitted,
            JournalBackupStatus::Verifying,
            backup_id,
        )
    }

    pub fn mark_backup_verified(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::Verifying,
            JournalBackupStatus::Verified,
            backup_id,
        )
    }

    pub fn mark_backup_catalog_writing(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::Verified,
            JournalBackupStatus::CatalogWriting,
            backup_id,
        )
    }

    pub fn mark_backup_written(&mut self, backup_id: Uuid) -> Result<(), CleanerError> {
        self.transition_backup(
            JournalBackupStatus::CatalogWriting,
            JournalBackupStatus::Committed,
            backup_id,
        )?;
        self.backup_id = Some(backup_id);
        self.status = OperationStatus::BackupWritten;
        self.touch();
        Ok(())
    }

    pub fn mark_mutation_started(&mut self, index: usize) -> Result<(), CleanerError> {
        if !matches!(
            self.status,
            OperationStatus::Planned | OperationStatus::BackupWritten | OperationStatus::Deleting
        ) {
            return Err(invalid_transition("deleting", self.status));
        }
        if self.create_backup && self.backup_id.is_none() {
            return Err(CleanerError::Blocked(
                "a backup-selected operation cannot mutate before backup commit".into(),
            ));
        }
        if self
            .mutations
            .iter()
            .take(index)
            .any(|mutation| mutation.status != JournalMutationStatus::Applied)
        {
            return Err(CleanerError::InvalidRequest(
                "mutation progress cannot skip an earlier operation".into(),
            ));
        }
        let mutation = self.mutations.get_mut(index).ok_or_else(|| {
            CleanerError::InvalidRequest(format!("unknown mutation index {index}"))
        })?;
        if mutation.status != JournalMutationStatus::Pending {
            return Err(CleanerError::InvalidRequest(format!(
                "mutation {index} is not pending"
            )));
        }
        let now = Utc::now();
        mutation.status = JournalMutationStatus::Running;
        mutation.started_at = Some(now);
        self.status = OperationStatus::Deleting;
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_mutation_applied(&mut self, index: usize) -> Result<(), CleanerError> {
        if self.status != OperationStatus::Deleting {
            return Err(invalid_transition("mutation applied", self.status));
        }
        let mutation = self.mutations.get_mut(index).ok_or_else(|| {
            CleanerError::InvalidRequest(format!("unknown mutation index {index}"))
        })?;
        if mutation.status != JournalMutationStatus::Running {
            return Err(CleanerError::InvalidRequest(format!(
                "mutation {index} is not running"
            )));
        }
        let now = Utc::now();
        mutation.status = JournalMutationStatus::Applied;
        mutation.applied_at = Some(now);
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_verification_started(&mut self) -> Result<(), CleanerError> {
        if self
            .mutations
            .iter()
            .any(|mutation| mutation.status != JournalMutationStatus::Applied)
        {
            return Err(CleanerError::Blocked(
                "verification cannot start while mutations remain unapplied".into(),
            ));
        }
        if self.status != OperationStatus::Deleting {
            return Err(invalid_transition("verifying", self.status));
        }
        self.status = OperationStatus::Verifying;
        self.touch();
        Ok(())
    }

    pub fn mark_verified(&mut self) -> Result<(), CleanerError> {
        if self
            .mutations
            .iter()
            .any(|mutation| mutation.status != JournalMutationStatus::Applied)
        {
            return Err(CleanerError::Blocked(
                "verification cannot complete while mutations remain unapplied".into(),
            ));
        }
        if !matches!(
            self.status,
            OperationStatus::Verifying | OperationStatus::Failed | OperationStatus::Verified
        ) {
            return Err(invalid_transition("verified", self.status));
        }
        self.status = OperationStatus::Verified;
        self.message = None;
        self.touch();
        Ok(())
    }

    pub fn mark_complete(&mut self) -> Result<(), CleanerError> {
        if self.status != OperationStatus::Verified {
            return Err(invalid_transition("complete", self.status));
        }
        self.status = OperationStatus::Complete;
        self.message = None;
        self.touch();
        Ok(())
    }

    pub fn mark_failed(&mut self, message: impl Into<String>) -> Result<(), CleanerError> {
        if self.is_terminal() {
            return Err(invalid_transition("failed", self.status));
        }
        self.status = OperationStatus::Failed;
        self.message = Some(message.into());
        self.touch();
        Ok(())
    }

    pub fn mark_recovered(&mut self, message: impl Into<String>) -> Result<(), CleanerError> {
        if !self.needs_recovery() {
            return Err(invalid_transition("recovered", self.status));
        }
        self.status = OperationStatus::Recovered;
        self.message = Some(message.into());
        self.touch();
        Ok(())
    }

    pub fn mark_reconciled_complete(
        &mut self,
        message: impl Into<String>,
    ) -> Result<(), CleanerError> {
        if !self.needs_recovery() {
            return Err(invalid_transition("reconciled complete", self.status));
        }
        self.status = OperationStatus::Complete;
        self.message = Some(message.into());
        self.touch();
        Ok(())
    }

    pub fn mark_terminated(&mut self, message: impl Into<String>) -> Result<(), CleanerError> {
        if !self.needs_recovery() {
            return Err(invalid_transition("terminated", self.status));
        }
        self.status = OperationStatus::Terminated;
        self.message = Some(message.into());
        self.touch();
        Ok(())
    }

    pub fn needs_recovery(&self) -> bool {
        !matches!(
            self.status,
            OperationStatus::Complete | OperationStatus::Recovered | OperationStatus::Terminated
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            OperationStatus::Complete | OperationStatus::Recovered | OperationStatus::Terminated
        )
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    fn transition_backup(
        &mut self,
        expected: JournalBackupStatus,
        next: JournalBackupStatus,
        backup_id: Uuid,
    ) -> Result<(), CleanerError> {
        if !self.create_backup
            || self.status != OperationStatus::Planned
            || self.backup_status != expected
        {
            return Err(CleanerError::InvalidRequest(format!(
                "backup journal cannot transition from {:?}/{:?} to {next:?}",
                self.status, self.backup_status
            )));
        }
        match self.backup_candidate_id {
            Some(existing) if existing != backup_id => {
                return Err(CleanerError::Blocked(
                    "backup identity changed during creation".into(),
                ));
            }
            Some(_) => {}
            None => self.backup_candidate_id = Some(backup_id),
        }
        self.backup_status = next;
        self.touch();
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), CleanerError> {
        if self.format_version != JOURNAL_FORMAT_VERSION {
            return Err(CleanerError::Unsupported(format!(
                "operation journal format {} is not recognized",
                self.format_version
            )));
        }
        if self.operation_id != self.plan.id || self.snapshot_id != self.plan.snapshot_id {
            return Err(CleanerError::InvalidRequest(
                "operation journal identity does not match its immutable plan".into(),
            ));
        }
        if self.mutations.len() != self.plan.operations.len() {
            return Err(CleanerError::InvalidRequest(
                "operation journal progress does not match its immutable plan".into(),
            ));
        }
        let planned_item_ids = self
            .plan
            .operations
            .iter()
            .flat_map(|operation| &operation.item_ids)
            .collect::<BTreeSet<_>>();
        if self
            .item_categories
            .keys()
            .any(|item_id| !planned_item_ids.contains(item_id))
        {
            return Err(CleanerError::InvalidRequest(
                "operation journal contains item metadata outside its immutable plan".into(),
            ));
        }
        for (index, (mutation, operation)) in
            self.mutations.iter().zip(&self.plan.operations).enumerate()
        {
            if mutation.index != index
                || mutation.kind != operation.kind
                || mutation.item_ids != operation.item_ids
                || mutation.session_ids != operation.session_ids
            {
                return Err(CleanerError::InvalidRequest(format!(
                    "mutation {index} does not match the immutable plan"
                )));
            }
        }
        if self.create_backup
            && matches!(
                self.status,
                OperationStatus::BackupWritten
                    | OperationStatus::Deleting
                    | OperationStatus::Verifying
                    | OperationStatus::Verified
                    | OperationStatus::Complete
            )
            && self.backup_id.is_none()
        {
            return Err(CleanerError::InvalidRequest(
                "backup-selected journal advanced without a backup identity".into(),
            ));
        }
        if !self.create_backup
            && (self.backup_status != JournalBackupStatus::NotRequested
                || self.backup_candidate_id.is_some()
                || self.backup_id.is_some())
        {
            return Err(CleanerError::InvalidRequest(
                "backup-disabled journal contains backup progress".into(),
            ));
        }
        if self.backup_status == JournalBackupStatus::Committed
            && (self.backup_candidate_id.is_none() || self.backup_candidate_id != self.backup_id)
        {
            return Err(CleanerError::InvalidRequest(
                "committed backup journal has inconsistent identities".into(),
            ));
        }
        Ok(())
    }

    fn immutable_matches(&self, next: &Self) -> bool {
        self.format_version == next.format_version
            && self.operation_id == next.operation_id
            && self.agent == next.agent
            && self.snapshot_id == next.snapshot_id
            && self.created_at == next.created_at
            && self.create_backup == next.create_backup
            && self.plan == next.plan
            && self.item_categories == next.item_categories
            && self.mutations.len() == next.mutations.len()
            && self
                .mutations
                .iter()
                .zip(&next.mutations)
                .all(|(left, right)| {
                    left.index == right.index
                        && left.kind == right.kind
                        && left.item_ids == right.item_ids
                        && left.session_ids == right.session_ids
                })
    }
}

#[derive(Debug, Clone, Default)]
pub struct JournalInventory {
    pub journals: Vec<OperationJournal>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct JournalStore {
    base_dir: PathBuf,
}

impl JournalStore {
    pub fn new(base_dir: PathBuf) -> Result<Self, CleanerError> {
        fs::create_dir_all(&base_dir)?;
        let base_dir = validate_existing_beneath(&base_dir, std::slice::from_ref(&base_dir))?;
        Ok(Self { base_dir })
    }

    pub fn create(&self, journal: &OperationJournal) -> Result<(), CleanerError> {
        journal.validate_shape()?;
        let path = self.path(journal.operation_id);
        if fs::symlink_metadata(&path).is_ok() {
            return Err(CleanerError::Blocked(format!(
                "operation journal already exists: {}",
                journal.operation_id
            )));
        }
        self.write_atomic(&path, journal)
    }

    pub fn save(&self, journal: &OperationJournal) -> Result<(), CleanerError> {
        journal.validate_shape()?;
        let previous = self.load(journal.operation_id)?;
        if !previous.immutable_matches(journal) {
            return Err(CleanerError::Blocked(
                "operation journal immutable plan changed".into(),
            ));
        }
        self.write_atomic(&self.path(journal.operation_id), journal)
    }

    pub fn load(&self, operation_id: Uuid) -> Result<OperationJournal, CleanerError> {
        let path = self.path(operation_id);
        let path = validate_existing_beneath(&path, std::slice::from_ref(&self.base_dir))?;
        let metadata = fs::metadata(&path)?;
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(CleanerError::Blocked(format!(
                "operation journal exceeds {MAX_JOURNAL_BYTES} bytes"
            )));
        }
        let journal: OperationJournal =
            serde_json::from_reader(BufReader::new(fs::File::open(path)?))?;
        journal.validate_shape()?;
        Ok(journal)
    }

    pub fn inventory(&self) -> Result<JournalInventory, CleanerError> {
        let mut inventory = JournalInventory::default();
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                // A sibling partial is never authoritative. If an older committed
                // journal exists it is loaded separately; if it does not, mutation
                // could not have started because initial journal creation precedes it.
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                inventory
                    .warnings
                    .push(format!("unrecognized operation journal name: {name}"));
                continue;
            };
            let Ok(operation_id) = Uuid::parse_str(stem) else {
                inventory
                    .warnings
                    .push(format!("unrecognized operation journal name: {name}"));
                continue;
            };
            match self.load(operation_id) {
                Ok(journal) => inventory.journals.push(journal),
                Err(error) => match self.remove_legacy(operation_id) {
                    Ok(true) => {}
                    Ok(false) => inventory.warnings.push(format!(
                        "operation journal {operation_id} is unavailable: {error}"
                    )),
                    Err(remove_error) => inventory.warnings.push(format!(
                        "obsolete operation journal {operation_id} could not be removed: {remove_error}"
                    )),
                },
            }
        }
        inventory.journals.sort_by_key(|journal| journal.created_at);
        Ok(inventory)
    }

    fn path(&self, operation_id: Uuid) -> PathBuf {
        self.base_dir.join(format!("{operation_id}.json"))
    }

    fn write_atomic(
        &self,
        destination: &Path,
        journal: &OperationJournal,
    ) -> Result<(), CleanerError> {
        let partial = self.base_dir.join(format!(
            ".{}.{}.partial",
            destination
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| CleanerError::UnsafePath(destination.display().to_string()))?,
            Uuid::new_v4()
        ));
        let result = (|| {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer_pretty(&mut writer, journal)?;
            writer.flush()?;
            writer
                .into_inner()
                .map_err(|error| error.into_error())?
                .sync_all()?;
            atomic_replace_file(&partial, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result
    }

    fn remove_legacy(&self, operation_id: Uuid) -> Result<bool, CleanerError> {
        let path = self.path(operation_id);
        let path = validate_existing_beneath(&path, std::slice::from_ref(&self.base_dir))?;
        let metadata = fs::metadata(&path)?;
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Ok(false);
        }
        let legacy: LegacyOperationJournal =
            match serde_json::from_reader(BufReader::new(fs::File::open(&path)?)) {
                Ok(legacy) => legacy,
                Err(_) => return Ok(false),
            };
        if legacy.operation_id != operation_id {
            return Ok(false);
        }
        let _ = (
            legacy.status,
            legacy.updated_at,
            legacy.backup_id,
            legacy.message,
        );
        fs::remove_file(&path)?;
        sync_parent_directory(&path)?;
        Ok(true)
    }
}

fn invalid_transition(target: &str, status: OperationStatus) -> CleanerError {
    CleanerError::InvalidRequest(format!(
        "operation journal cannot transition from {status:?} to {target}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlannedOperation;
    use chrono::Utc;

    fn plan(operation_count: usize) -> CleanupPlan {
        CleanupPlan {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            created_at: Utc::now(),
            selected_item_ids: (0..operation_count)
                .map(|index| format!("item-{index}"))
                .collect(),
            expanded_session_ids: Vec::new(),
            operations: (0..operation_count)
                .map(|index| PlannedOperation {
                    kind: OperationKind::CleanRegenerable,
                    item_ids: vec![format!("item-{index}")],
                    session_ids: Vec::new(),
                    paths: vec![format!("/fixture/cache-{index}")],
                    size_bytes: 1,
                    backup_eligible: true,
                    requires_agent_exit: true,
                    blockers: Vec::new(),
                })
                .collect(),
            estimated_bytes: operation_count as u64,
            estimated_backup_bytes: operation_count as u64,
            blockers: Vec::new(),
        }
    }

    #[test]
    fn journal_reopens_at_every_mutation_boundary() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let store = JournalStore::new(directory.path().join("operations")).expect("store");
        let mut journal = OperationJournal::new(plan(2), AgentKind::Pi, true);
        store.create(&journal).expect("planned journal");
        assert_eq!(
            store.load(journal.operation_id).expect("reopen planned"),
            journal
        );

        let backup_id = Uuid::new_v4();
        journal
            .mark_backup_writing(backup_id)
            .expect("backup writing");
        journal
            .mark_backup_archive_committed(backup_id)
            .expect("archive committed");
        journal
            .mark_backup_verifying(backup_id)
            .expect("backup verifying");
        journal
            .mark_backup_verified(backup_id)
            .expect("backup verified");
        journal
            .mark_backup_catalog_writing(backup_id)
            .expect("catalog writing");
        journal
            .mark_backup_written(backup_id)
            .expect("backup committed");
        store.save(&journal).expect("save backup");
        for index in 0..2 {
            journal
                .mark_mutation_started(index)
                .expect("start mutation");
            store.save(&journal).expect("save before mutation");
            assert_eq!(
                store
                    .load(journal.operation_id)
                    .expect("reopen before mutation")
                    .mutations[index]
                    .status,
                JournalMutationStatus::Running
            );

            journal
                .mark_mutation_applied(index)
                .expect("apply mutation");
            store.save(&journal).expect("save after mutation");
            assert_eq!(
                store
                    .load(journal.operation_id)
                    .expect("reopen after mutation")
                    .mutations[index]
                    .status,
                JournalMutationStatus::Applied
            );
        }
        journal
            .mark_verification_started()
            .expect("verification started");
        store.save(&journal).expect("save verifying");
        journal.mark_verified().expect("verified");
        store.save(&journal).expect("save verified");
        journal.mark_complete().expect("complete");
        store.save(&journal).expect("save complete");
        assert!(
            !store
                .load(journal.operation_id)
                .expect("completed journal")
                .needs_recovery()
        );
    }

    #[test]
    fn journal_rejects_changes_to_the_immutable_plan() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let store = JournalStore::new(directory.path().join("operations")).expect("store");
        let mut journal = OperationJournal::new(plan(1), AgentKind::ClaudeCode, false);
        store.create(&journal).expect("planned journal");
        journal.plan.operations[0].paths[0] = "/fixture/changed".into();
        assert!(matches!(
            store.save(&journal),
            Err(CleanerError::Blocked(_))
        ));
    }

    #[test]
    fn reconciliation_preserves_the_last_durable_mutation_progress() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let store = JournalStore::new(directory.path().join("operations")).expect("store");
        let mut journal = OperationJournal::new(plan(1), AgentKind::OpenCode, false);
        store.create(&journal).expect("planned journal");
        journal.mark_mutation_started(0).expect("running mutation");
        store.save(&journal).expect("crash boundary");

        let mut reopened = store.load(journal.operation_id).expect("startup reopen");
        reopened
            .mark_reconciled_complete("rescan proved the planned effect")
            .expect("reconciled");
        store.save(&reopened).expect("save reconciliation");

        let completed = store.load(journal.operation_id).expect("completed journal");
        assert_eq!(completed.status, OperationStatus::Complete);
        assert_eq!(
            completed.mutations[0].status,
            JournalMutationStatus::Running,
            "claimed progress remains distinct from recovery observation"
        );
        assert!(
            completed
                .message
                .as_deref()
                .is_some_and(|message| message.contains("rescan proved"))
        );
    }

    #[test]
    fn incomplete_partial_journal_is_never_loaded_as_committed() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let operations = directory.path().join("operations");
        let store = JournalStore::new(operations.clone()).expect("store");
        fs::write(
            operations.join("orphan.json.partial"),
            b"private partial bytes",
        )
        .expect("partial");
        let inventory = store.inventory().expect("inventory");
        assert!(inventory.journals.is_empty());
        assert!(inventory.warnings.is_empty());
    }

    #[test]
    fn legacy_journals_are_removed_during_inventory() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let operations = directory.path().join("operations");
        let store = JournalStore::new(operations.clone()).expect("store");
        let completed = Uuid::new_v4();
        let failed = Uuid::new_v4();
        let incomplete = Uuid::new_v4();
        for (operation_id, status) in [
            (completed, OperationStatus::Complete),
            (failed, OperationStatus::Failed),
            (incomplete, OperationStatus::Deleting),
        ] {
            fs::write(
                operations.join(format!("{operation_id}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "operationId": operation_id,
                    "status": status,
                    "updatedAt": Utc::now(),
                    "backupId": null,
                    "message": null
                }))
                .expect("legacy journal"),
            )
            .expect("write legacy journal");
        }

        let inventory = store.inventory().expect("inventory");

        assert!(inventory.journals.is_empty());
        assert!(inventory.warnings.is_empty());
        for operation_id in [completed, failed, incomplete] {
            assert!(!operations.join(format!("{operation_id}.json")).exists());
        }
    }

    #[test]
    fn unknown_committed_journal_is_not_removed_as_legacy() {
        let directory = tempfile::tempdir().expect("journal fixture");
        let operations = directory.path().join("operations");
        let store = JournalStore::new(operations.clone()).expect("store");
        let operation_id = Uuid::new_v4();
        let path = operations.join(format!("{operation_id}.json"));
        let bytes = serde_json::to_vec(&serde_json::json!({
            "formatVersion": 99,
            "operationId": operation_id,
            "status": OperationStatus::Failed,
            "updatedAt": Utc::now(),
            "backupId": null,
            "message": "future journal"
        }))
        .expect("future journal");
        fs::write(&path, &bytes).expect("write future journal");

        let inventory = store.inventory().expect("inventory");

        assert!(inventory.journals.is_empty());
        assert_eq!(inventory.warnings.len(), 1);
        assert_eq!(fs::read(path).expect("future journal preserved"), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn redirecting_journal_is_rejected_without_reading_external_bytes() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("journal fixture");
        let operations = directory.path().join("operations");
        let store = JournalStore::new(operations.clone()).expect("store");
        let external = directory.path().join("protected.json");
        fs::write(&external, b"protected journal bytes").expect("protected bytes");
        let operation_id = Uuid::new_v4();
        symlink(&external, operations.join(format!("{operation_id}.json"))).expect("redirect");

        assert!(matches!(
            store.load(operation_id),
            Err(CleanerError::UnsafePath(_))
        ));
        assert_eq!(
            fs::read(external).expect("protected bytes unchanged"),
            b"protected journal bytes"
        );
    }
}
