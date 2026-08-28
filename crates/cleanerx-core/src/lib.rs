//! Shared domain, safety, and backup primitives for CleanerX.

pub mod backup;
pub mod journal;
pub mod model;
pub mod planner;
pub mod platform;
pub mod safety;

pub use backup::{BackupEvent, BackupSource, BackupStore};
pub use journal::{
    JOURNAL_FORMAT_VERSION, JournalBackupStatus, JournalInventory, JournalMutation,
    JournalMutationStatus, JournalStore, OperationJournal,
};
pub use model::*;
pub use planner::create_cleanup_plan;
pub use platform::{atomic_replace_file, configure_background_command};
pub use safety::{
    FileIdentity, PathPolicy, metadata_revision, safe_remove, validate_existing_beneath,
    validate_new_file_destination,
};

use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Compile-time extension point for coding-agent storage adapters.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    async fn detect(&self, custom_home: Option<&str>) -> Result<AgentInstallation, CleanerError>;
    async fn scan(&self, custom_home: Option<&str>) -> Result<InventorySnapshot, CleanerError>;
    async fn load_item_content(
        &self,
        installation: &AgentInstallation,
        item: &CleanupItem,
    ) -> Result<ItemContentDetail, CleanerError>;
    async fn load_item_thumbnail(
        &self,
        installation: &AgentInstallation,
        item: &CleanupItem,
    ) -> Result<Option<ItemThumbnail>, CleanerError>;
    async fn delete_sessions(
        &self,
        installation: &AgentInstallation,
        session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError>;
    async fn reset_memory(&self, installation: &AgentInstallation) -> Result<(), CleanerError>;

    /// Exports complete session records through the Agent's supported public route.
    /// The returned files must live directly beneath `destination`.
    async fn export_sessions(
        &self,
        _installation: &AgentInstallation,
        _session_ids: &[String],
        _destination: &Path,
    ) -> Result<Vec<PathBuf>, CleanerError> {
        Err(CleanerError::Unsupported(
            "session export is not supported by this Agent".into(),
        ))
    }

    /// Restores previously exported sessions through the Agent's supported public route.
    async fn import_sessions(
        &self,
        _installation: &AgentInstallation,
        _exports: &[PathBuf],
    ) -> Result<Vec<String>, CleanerError> {
        Err(CleanerError::Unsupported(
            "session import is not supported by this Agent".into(),
        ))
    }
}
