use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum CleanerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsafe path rejected: {0}")]
    UnsafePath(String),
    #[error("operation is blocked: {0}")]
    Blocked(String),
    #[error("Agent integration error: {0}")]
    Integration(String),
    #[error("backup error: {0}")]
    Backup(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsupported Agent capability: {0}")]
    Unsupported(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    #[default]
    Codex,
    ClaudeCode,
    OpenCode,
}

impl AgentKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::OpenCode => "OpenCode",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemoryScope {
    Global,
    Project,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCapabilities {
    pub can_scan: bool,
    pub can_read_content: bool,
    pub can_reset_all: bool,
    pub can_reset_scope: bool,
    pub can_edit_entries: bool,
    pub can_delete_entries: bool,
    pub can_toggle_use: bool,
    pub can_toggle_generation: bool,
    pub scope: MemoryScope,
}

impl Default for MemoryCapabilities {
    fn default() -> Self {
        Self {
            can_scan: false,
            can_read_content: false,
            can_reset_all: false,
            can_reset_scope: false,
            can_edit_entries: false,
            can_delete_entries: false,
            can_toggle_use: false,
            can_toggle_generation: false,
            scope: MemoryScope::Global,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum StorageCategory {
    Session,
    ArchivedSession,
    Memory,
    Attachment,
    GeneratedImage,
    Log,
    Cache,
    Temporary,
    Protected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    Safe,
    Review,
    High,
    Protected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    DeleteSession,
    ResetMemory,
    CleanRegenerable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Planned,
    BackupWritten,
    Deleting,
    Verified,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub thread_list: bool,
    pub thread_delete: bool,
    pub memory: MemoryCapabilities,
    pub descendant_filter: bool,
    pub report_only: bool,
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self {
            thread_list: false,
            thread_delete: false,
            memory: MemoryCapabilities::default(),
            descendant_filter: false,
            report_only: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstallation {
    pub kind: AgentKind,
    pub home: String,
    pub binary: Option<String>,
    pub version: Option<String>,
    pub app_support: Option<String>,
    pub running: bool,
    pub capabilities: AgentCapabilities,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItem {
    pub id: String,
    pub category: StorageCategory,
    pub title: String,
    pub subtitle: Option<String>,
    pub paths: Vec<String>,
    pub project_id: Option<String>,
    pub thread_id: Option<String>,
    pub size_bytes: u64,
    pub modified_at: Option<DateTime<Utc>>,
    pub risk: RiskLevel,
    pub recoverable: bool,
    pub default_selected: bool,
    pub protected: bool,
    pub blocked_reason: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ContentBlock {
    Message {
        role: String,
        text: String,
        phase: Option<String>,
    },
    Text {
        title: String,
        text: String,
    },
    Image {
        title: String,
        data_url: String,
    },
    Log {
        timestamp: Option<DateTime<Utc>>,
        level: Option<String>,
        target: Option<String>,
        text: String,
    },
    Notice {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemContentDetail {
    pub item_id: String,
    pub source: String,
    pub truncated: bool,
    pub bytes_read: u64,
    pub blocks: Vec<ContentBlock>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemThumbnail {
    pub item_id: String,
    pub title: String,
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub path: Option<String>,
    pub source: String,
    pub archived: bool,
    pub pinned: bool,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub size_bytes: u64,
    pub parent_thread_id: Option<String>,
    pub descendant_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    pub roots: Vec<String>,
    pub session_ids: Vec<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySummary {
    pub category: StorageCategory,
    pub size_bytes: u64,
    pub item_count: usize,
    pub default_selected_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventorySnapshot {
    pub id: Uuid,
    pub scanned_at: DateTime<Utc>,
    pub installation: AgentInstallation,
    pub total_bytes: u64,
    pub reclaimable_bytes: u64,
    pub items: Vec<CleanupItem>,
    pub sessions: Vec<SessionRecord>,
    pub projects: Vec<ProjectGroup>,
    pub categories: Vec<CategorySummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedOperation {
    pub kind: OperationKind,
    pub item_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub paths: Vec<String>,
    pub size_bytes: u64,
    pub backup_eligible: bool,
    pub requires_agent_exit: bool,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    pub id: Uuid,
    pub snapshot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub selected_item_ids: Vec<String>,
    pub expanded_session_ids: Vec<String>,
    pub operations: Vec<PlannedOperation>,
    pub estimated_bytes: u64,
    pub estimated_backup_bytes: u64,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub root: String,
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format_version: u32,
    pub id: Uuid,
    pub agent: AgentKind,
    pub agent_version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub operation_id: Uuid,
    pub item_count: usize,
    pub original_bytes: u64,
    pub archive_bytes: u64,
    pub entries: Vec<BackupEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub archive_path: String,
    pub archive_bytes: u64,
    pub original_bytes: u64,
    pub item_count: usize,
    pub operation_id: Uuid,
    #[serde(default)]
    pub agent: AgentKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub operation_id: Uuid,
    pub status: OperationStatus,
    pub backup_id: Option<Uuid>,
    pub reclaimed_bytes: u64,
    pub deleted_item_ids: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AppSettings {
    pub active_agent: AgentKind,
    pub custom_codex_home: Option<String>,
    pub custom_claude_home: Option<String>,
    pub custom_opencode_home: Option<String>,
    pub locale: String,
    pub theme: String,
    pub text_size: String,
    pub backup_retention_days: u32,
    pub log_retention_days: u32,
    pub temp_retention_hours: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            active_agent: AgentKind::Codex,
            custom_codex_home: None,
            custom_claude_home: None,
            custom_opencode_home: None,
            locale: "system".into(),
            theme: "system".into(),
            text_size: "large".into(),
            backup_retention_days: 30,
            log_retention_days: 7,
            temp_retention_hours: 24,
        }
    }
}
