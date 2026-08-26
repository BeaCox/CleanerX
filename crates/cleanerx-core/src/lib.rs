//! Shared domain, safety, and backup primitives for CleanerX.

pub mod backup;
pub mod model;
pub mod planner;
pub mod safety;

pub use backup::{BackupSource, BackupStore};
pub use model::*;
pub use planner::create_cleanup_plan;
pub use safety::{FileIdentity, PathPolicy, safe_remove};

use async_trait::async_trait;

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
    async fn delete_sessions(
        &self,
        installation: &AgentInstallation,
        session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError>;
    async fn reset_memory(&self, installation: &AgentInstallation) -> Result<(), CleanerError>;
}
