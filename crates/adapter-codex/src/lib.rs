//! Codex storage discovery and App Server integration.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead as _, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration as StdDuration, SystemTime};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentCapabilities, AgentInstallation, AgentKind, CategorySummary, CleanerError,
    CleanupItem, ContentBlock, InventorySnapshot, ItemContentDetail, ItemThumbnail, ProjectGroup,
    RiskLevel, SessionRecord, StorageCategory,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use uuid::Uuid;
use walkdir::WalkDir;

const CONTENT_TEXT_LIMIT: usize = 512 * 1024;
const CONTENT_IMAGE_LIMIT: u64 = 5 * 1024 * 1024;
const CONTENT_BLOCK_LIMIT: usize = 200;

const SOURCE_KINDS: &[&str] = &[
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

const PROTECTED_NAMES: &[&str] = &[
    "auth.json",
    "config.toml",
    "rules",
    "skills",
    "plugins",
    "state",
    "installation_id",
];

#[derive(Debug, Clone, Default)]
pub struct CodexAdapter {
    bypass_unresponsive_socket: Arc<AtomicBool>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve_home(&self, custom_home: Option<&str>) -> Result<PathBuf, CleanerError> {
        let path = custom_home
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("CODEX_HOME").map(PathBuf::from))
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
            .ok_or_else(|| CleanerError::NotFound("Codex home directory".into()))?;
        if !path.is_absolute() {
            return Err(CleanerError::InvalidRequest(
                "Codex home override must be an absolute path".into(),
            ));
        }
        Ok(path)
    }

    async fn app_client(
        &self,
        binary: &Path,
        home: &Path,
    ) -> Result<AppServerClient, CleanerError> {
        let mut client = AppServerClient::connect(
            binary,
            home,
            self.bypass_unresponsive_socket.load(Ordering::Relaxed),
        )
        .await?;
        if client.connection_warning.is_some() {
            self.bypass_unresponsive_socket
                .store(true, Ordering::Relaxed);
        } else if self.bypass_unresponsive_socket.load(Ordering::Relaxed) {
            client.connection_warning = Some(
                "CleanerX is using a temporary local App Server because the Codex control socket was unavailable earlier in this run".into(),
            );
        }
        Ok(client)
    }
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    async fn detect(&self, custom_home: Option<&str>) -> Result<AgentInstallation, CleanerError> {
        let home = self.resolve_home(custom_home)?;
        let binary = find_codex_binary();
        let version = if let Some(binary) = &binary {
            Command::new(binary)
                .arg("--version")
                .output()
                .await
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            None
        };
        let running = codex_is_running(&home);
        let app_support = codex_app_support();
        let mut warnings = Vec::new();
        if !home.exists() {
            warnings.push(format!("Codex home does not exist: {}", home.display()));
        }
        if binary.is_none() {
            warnings.push("Codex executable was not found; write operations are disabled".into());
        }

        let mut capabilities = AgentCapabilities::default();
        if let Some(binary) = &binary {
            match self.app_client(binary, &home).await {
                Ok(mut client) => {
                    if let Some(warning) = client.connection_warning.take() {
                        warnings.push(warning);
                    }
                    capabilities.thread_list = true;
                    capabilities.descendant_filter = client
                        .request(
                            "thread/list",
                            json!({
                                "ancestorThreadId": Uuid::nil().to_string(),
                                "limit": 1,
                                "sourceKinds": SOURCE_KINDS,
                                "useStateDbOnly": true
                            }),
                        )
                        .await
                        .is_ok();
                    capabilities.thread_delete = true;
                    capabilities.memory_reset = client.supports_memory_reset().await;
                    capabilities.report_only = false;
                }
                Err(error) => warnings.push(format!(
                    "App Server unavailable; using read-only storage scan: {error}"
                )),
            }
        }

        Ok(AgentInstallation {
            kind: AgentKind::Codex,
            home: home.to_string_lossy().into_owned(),
            binary: binary.map(|path| path.to_string_lossy().into_owned()),
            version,
            app_support: app_support.map(|path| path.to_string_lossy().into_owned()),
            running,
            capabilities,
            warnings,
        })
    }

    async fn scan(&self, custom_home: Option<&str>) -> Result<InventorySnapshot, CleanerError> {
        let mut installation = self.detect(custom_home).await?;
        let home = PathBuf::from(&installation.home);
        let app_support = installation.app_support.as_deref().map(PathBuf::from);
        let mut warnings = installation.warnings.clone();

        let sessions_result = if let Some(binary) = installation.binary.as_deref() {
            match self.app_client(Path::new(binary), &home).await {
                Ok(mut client) => scan_sessions_from_server(&mut client).await,
                Err(error) => Err(error),
            }
        } else {
            Err(CleanerError::Integration(
                "Codex executable not available".into(),
            ))
        };

        let mut sessions = match sessions_result {
            Ok(sessions) => sessions,
            Err(error) => {
                warnings.push(format!(
                    "Session API scan failed; report is read-only and based on a validated state DB: {error}"
                ));
                installation.capabilities = AgentCapabilities::default();
                scan_sessions_from_state_db(&home)?
            }
        };
        augment_pinned_and_parents(&home, &mut sessions, &mut warnings);
        populate_descendants(&mut sessions);

        let mut items = Vec::new();
        for session in &mut sessions {
            let mut paths = Vec::new();
            if let Some(path) = &session.path
                && let Ok(size) = cleanerx_core::safety::allocated_size(Path::new(path))
            {
                session.size_bytes = session.size_bytes.saturating_add(size);
                paths.push(path.clone());
            }
            for associated in associated_paths(&home, &session.id) {
                if let Ok(size) = cleanerx_core::safety::allocated_size(&associated) {
                    session.size_bytes = session.size_bytes.saturating_add(size);
                    paths.push(associated.to_string_lossy().into_owned());
                }
            }
            let blocked_reason = if is_active_status(&session.status) {
                Some("Thread is active or loaded in Codex".into())
            } else {
                None
            };
            let category = if session.archived {
                StorageCategory::ArchivedSession
            } else {
                StorageCategory::Session
            };
            items.push(CleanupItem {
                id: format!("session:{}", session.id),
                category,
                title: session.name.clone(),
                subtitle: Some(session.cwd.clone()),
                paths,
                project_id: Some(project_id_for_cwd(&session.cwd)),
                thread_id: Some(session.id.clone()),
                size_bytes: session.size_bytes,
                modified_at: session.updated_at,
                risk: RiskLevel::High,
                recoverable: true,
                default_selected: false,
                protected: false,
                blocked_reason,
                metadata: BTreeMap::from([
                    ("source".into(), session.source.clone()),
                    ("status".into(), session.status.clone()),
                    ("pinned".into(), session.pinned.to_string()),
                ]),
            });
        }

        scan_memory(&home, &mut items)?;
        scan_orphan_generated_content(&home, &sessions, &mut items)?;
        scan_logs(&home, &mut items)?;
        scan_caches(
            &home,
            app_support.as_deref(),
            installation.running,
            &mut items,
        )?;
        scan_temporary(
            &home,
            app_support.as_deref(),
            installation.running,
            &mut items,
        )?;
        scan_protected(&home, &mut items)?;

        let projects = group_projects(&sessions, &home, &mut warnings);
        let categories = summarize_categories(&items);
        let total_bytes = items
            .iter()
            .filter(|item| item.category != StorageCategory::Protected)
            .map(|item| item.size_bytes)
            .sum();
        let reclaimable_bytes = items
            .iter()
            .filter(|item| item.default_selected && item.blocked_reason.is_none())
            .map(|item| item.size_bytes)
            .sum();

        Ok(InventorySnapshot {
            id: Uuid::new_v4(),
            scanned_at: Utc::now(),
            installation,
            total_bytes,
            reclaimable_bytes,
            items,
            sessions,
            projects,
            categories,
            warnings,
        })
    }

    async fn load_item_content(
        &self,
        installation: &AgentInstallation,
        item: &CleanupItem,
    ) -> Result<ItemContentDetail, CleanerError> {
        validate_content_paths(installation, item)?;
        if item.protected || item.category == StorageCategory::Protected {
            return Ok(content_notice(
                item,
                "protected",
                "CleanerX never opens the contents of protected authentication or configuration data.",
            ));
        }

        if matches!(
            item.category,
            StorageCategory::Session | StorageCategory::ArchivedSession
        ) {
            if let (Some(binary), Some(thread_id)) =
                (installation.binary.as_deref(), item.thread_id.as_deref())
                && let Ok(mut client) = self
                    .app_client(Path::new(binary), Path::new(&installation.home))
                    .await
                && let Ok(result) = client
                    .request(
                        "thread/read",
                        json!({ "threadId": thread_id, "includeTurns": true }),
                    )
                    .await
            {
                return Ok(content_from_thread_read(item, &result));
            }
            return content_from_rollout(item);
        }

        match item.category {
            StorageCategory::Memory => content_from_memory(item),
            StorageCategory::Log => content_from_logs(item),
            StorageCategory::Attachment
            | StorageCategory::GeneratedImage
            | StorageCategory::Cache
            | StorageCategory::Temporary => content_from_paths(item),
            _ => Ok(content_notice(
                item,
                "unsupported",
                "No preview is available for this item.",
            )),
        }
    }

    async fn load_item_thumbnail(
        &self,
        installation: &AgentInstallation,
        item: &CleanupItem,
    ) -> Result<Option<ItemThumbnail>, CleanerError> {
        if !matches!(
            item.category,
            StorageCategory::Attachment | StorageCategory::GeneratedImage
        ) {
            return Err(CleanerError::Unsupported(
                "thumbnails are available only for attachments and generated images".into(),
            ));
        }
        validate_content_paths(installation, item)?;
        thumbnail_from_paths(item)
    }

    async fn delete_sessions(
        &self,
        installation: &AgentInstallation,
        session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError> {
        if installation.capabilities.report_only || !installation.capabilities.thread_delete {
            return Err(CleanerError::Unsupported(
                "official thread/delete is unavailable".into(),
            ));
        }
        let binary = installation
            .binary
            .as_deref()
            .ok_or_else(|| CleanerError::NotFound("Codex executable".into()))?;
        let mut client = self
            .app_client(Path::new(binary), Path::new(&installation.home))
            .await?;
        let mut deleted = Vec::new();
        for session_id in session_ids {
            client
                .request("thread/delete", json!({ "threadId": session_id }))
                .await?;
            deleted.push(session_id.clone());
        }
        Ok(deleted)
    }

    async fn reset_memory(&self, installation: &AgentInstallation) -> Result<(), CleanerError> {
        if installation.running {
            return Err(CleanerError::Blocked(
                "Quit Codex before resetting global memory".into(),
            ));
        }
        if !installation.capabilities.memory_reset {
            return Err(CleanerError::Unsupported(
                "memory/reset is not exposed by this Codex version".into(),
            ));
        }
        let binary = installation
            .binary
            .as_deref()
            .ok_or_else(|| CleanerError::NotFound("Codex executable".into()))?;
        let mut client = self
            .app_client(Path::new(binary), Path::new(&installation.home))
            .await?;
        client.request("memory/reset", Value::Null).await?;
        Ok(())
    }
}

fn validate_content_paths(
    installation: &AgentInstallation,
    item: &CleanupItem,
) -> Result<(), CleanerError> {
    let mut roots = vec![PathBuf::from(&installation.home)];
    if let Some(path) = &installation.app_support {
        roots.push(PathBuf::from(path));
    }
    let roots: Vec<_> = roots
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    for raw_path in &item.paths {
        let path = Path::new(raw_path);
        if !path.exists() {
            continue;
        }
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(CleanerError::UnsafePath(raw_path.clone()));
        }
        let canonical = path.canonicalize()?;
        if !roots.iter().any(|root| canonical.starts_with(root)) {
            return Err(CleanerError::UnsafePath(raw_path.clone()));
        }
    }
    Ok(())
}

fn content_notice(item: &CleanupItem, source: &str, text: &str) -> ItemContentDetail {
    ItemContentDetail {
        item_id: item.id.clone(),
        source: source.into(),
        truncated: false,
        bytes_read: 0,
        blocks: vec![ContentBlock::Notice { text: text.into() }],
        warning: None,
    }
}

fn content_from_thread_read(item: &CleanupItem, result: &Value) -> ItemContentDetail {
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "appServer.thread/read".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    if let Some(turns) = result.pointer("/thread/turns").and_then(Value::as_array) {
        for turn in turns {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            for thread_item in items {
                render_thread_item(thread_item, &mut detail);
                if detail.blocks.len() >= CONTENT_BLOCK_LIMIT {
                    detail.truncated = true;
                    break;
                }
            }
            if detail.truncated {
                break;
            }
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "This session has no renderable persisted turns.".into(),
        });
    }
    detail
}

fn render_thread_item(value: &Value, detail: &mut ItemContentDetail) {
    if detail.blocks.len() >= CONTENT_BLOCK_LIMIT
        || detail.bytes_read as usize >= CONTENT_TEXT_LIMIT
    {
        detail.truncated = true;
        return;
    }
    let item_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match item_type {
        "userMessage" | "message"
            if item_type == "userMessage"
                || value.get("role").and_then(Value::as_str) == Some("user") =>
        {
            let text = content_text(value.get("content"));
            push_message(detail, "user", text, None);
        }
        "agentMessage" => push_message(
            detail,
            "assistant",
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            value
                .get("phase")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "message" => {
            let role = value.get("role").and_then(Value::as_str).unwrap_or("other");
            push_message(detail, role, content_text(value.get("content")), None);
        }
        "plan" => push_text_block(
            detail,
            "Plan",
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
        "reasoning" => {
            let summary = content_text(value.get("summary"));
            if !summary.is_empty() {
                push_text_block(detail, "Reasoning summary", summary);
            }
        }
        "commandExecution" => {
            let command = display_json_field(value.get("command"));
            let output = value
                .get("aggregatedOutput")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = if output.is_empty() {
                command
            } else {
                format!("$ {command}\n\n{output}")
            };
            push_text_block(detail, "Command", body);
        }
        "fileChange" => push_text_block(
            detail,
            "File changes",
            pretty_json(value.get("changes").unwrap_or(&Value::Null)),
        ),
        "mcpToolCall" | "dynamicToolCall" | "collabToolCall" => {
            let label = value
                .get("tool")
                .or_else(|| value.get("server"))
                .and_then(Value::as_str)
                .unwrap_or(item_type);
            push_text_block(detail, &format!("Tool · {label}"), pretty_json(value));
        }
        "webSearch" => push_text_block(
            detail,
            "Web search",
            value
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
        "enteredReviewMode" | "exitedReviewMode" => push_text_block(
            detail,
            "Review",
            value
                .get("review")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
        _ => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                push_text_block(detail, item_type, text.into());
            }
        }
    }
}

fn content_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        part.get("path")
                            .and_then(Value::as_str)
                            .map(|path| format!("[Local image: {path}]"))
                    })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(value) => value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        None => String::new(),
    }
}

fn push_message(detail: &mut ItemContentDetail, role: &str, text: String, phase: Option<String>) {
    if text.trim().is_empty() {
        return;
    }
    let text = bounded_text(detail, text);
    detail.blocks.push(ContentBlock::Message {
        role: role.into(),
        text,
        phase,
    });
}

fn push_text_block(detail: &mut ItemContentDetail, title: &str, text: String) {
    if text.trim().is_empty() {
        return;
    }
    let text = bounded_text(detail, text);
    detail.blocks.push(ContentBlock::Text {
        title: title.into(),
        text,
    });
}

fn bounded_text(detail: &mut ItemContentDetail, mut text: String) -> String {
    let remaining = CONTENT_TEXT_LIMIT.saturating_sub(detail.bytes_read as usize);
    if text.len() > remaining {
        let mut boundary = remaining.min(text.len());
        while boundary > 0 && !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n…");
        detail.truncated = true;
    }
    detail.bytes_read = detail.bytes_read.saturating_add(text.len() as u64);
    text
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn display_json_field(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn content_from_rollout(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let Some(path) = item.paths.iter().map(Path::new).find(|path| {
        path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
    }) else {
        return Ok(content_notice(
            item,
            "rollout",
            "The session rollout is unavailable for preview.",
        ));
    };
    let file = fs::File::open(path)?;
    let reader: Box<dyn std::io::BufRead> =
        if path.extension().and_then(|value| value.to_str()) == Some("zst") {
            Box::new(std::io::BufReader::new(zstd::stream::read::Decoder::new(
                file,
            )?))
        } else {
            Box::new(std::io::BufReader::new(file))
        };
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "rollout.readOnlyFallback".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: Some("Official thread/read was unavailable; this preview was parsed read-only from a recognized rollout file.".into()),
    };
    for line in reader.lines().take(2_000) {
        let line = line?;
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            render_rollout_event(&value, &mut detail);
        }
        if detail.blocks.len() >= CONTENT_BLOCK_LIMIT
            || detail.bytes_read as usize >= CONTENT_TEXT_LIMIT
        {
            detail.truncated = true;
            break;
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No renderable messages were found in this rollout.".into(),
        });
    }
    Ok(detail)
}

fn render_rollout_event(value: &Value, detail: &mut ItemContentDetail) {
    let payload = value.get("payload").unwrap_or(value);
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "user_message" => push_message(
            detail,
            "user",
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            None,
        ),
        "agent_message" => push_message(
            detail,
            "assistant",
            payload
                .get("message")
                .or_else(|| payload.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            None,
        ),
        "message" | "userMessage" | "agentMessage" | "plan" | "reasoning" | "commandExecution"
        | "fileChange" | "mcpToolCall" | "dynamicToolCall" | "collabToolCall" | "webSearch" => {
            render_thread_item(payload, detail)
        }
        _ => {}
    }
}

fn content_from_memory(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "recognizedMemoryDb.readOnly".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    let database = item
        .paths
        .iter()
        .map(Path::new)
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("memories_") && name.ends_with(".sqlite"))
        })
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
    if let Some(path) = database {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if require_columns(
            &connection,
            "stage1_outputs",
            &[
                "thread_id",
                "raw_memory",
                "rollout_summary",
                "rollout_slug",
                "generated_at",
            ],
        )
        .is_err()
        {
            detail.warning = Some(
                "The memory database schema is not recognized; its contents were not opened."
                    .into(),
            );
        } else {
            let mut statement = connection.prepare(
                "SELECT thread_id, raw_memory, rollout_summary, rollout_slug, generated_at FROM stage1_outputs ORDER BY generated_at DESC LIMIT 100",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let (thread_id, raw_memory, summary, slug, generated_at) = row?;
                let title = slug
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| thread_id.clone());
                let timestamp = DateTime::from_timestamp(generated_at, 0)
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| generated_at.to_string());
                push_text_block(
                    &mut detail,
                    &format!("{title} · {timestamp}"),
                    format!("{summary}\n\n{raw_memory}"),
                );
                if detail.blocks.len() >= CONTENT_BLOCK_LIMIT || detail.truncated {
                    break;
                }
            }
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No renderable memory entries were found.".into(),
        });
    }
    Ok(detail)
}

fn content_from_logs(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "recognizedLogDb.readOnly".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    let Some(path) = item.paths.iter().map(Path::new).find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("logs_") && name.ends_with(".sqlite"))
    }) else {
        return Ok(content_notice(
            item,
            "logs",
            "The log database is unavailable for preview.",
        ));
    };
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    if require_columns(
        &connection,
        "logs",
        &[
            "id",
            "ts",
            "level",
            "target",
            "feedback_log_body",
            "module_path",
            "file",
            "line",
            "thread_id",
        ],
    )
    .is_err()
    {
        return Ok(content_notice(
            item,
            "logs",
            "The log database schema is not recognized; its contents were not opened.",
        ));
    }
    let mut statement = connection.prepare(
        "SELECT ts, level, target, feedback_log_body, module_path, file, line, thread_id FROM logs ORDER BY id DESC LIMIT 200",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    for row in rows {
        let (ts, level, target, body, module, file, line, thread_id) = row?;
        let context = [
            module,
            file.map(|file| line.map_or(file.clone(), |line| format!("{file}:{line}"))),
            thread_id.map(|id| format!("thread {id}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let text = match (body, context.is_empty()) {
            (Some(body), true) => body,
            (Some(body), false) => format!("{body}\n{context}"),
            (None, _) => context,
        };
        let timestamp = DateTime::from_timestamp(ts, 0);
        let text = bounded_text(&mut detail, text);
        detail.blocks.push(ContentBlock::Log {
            timestamp,
            level: Some(level),
            target: Some(target),
            text,
        });
        if detail.blocks.len() >= CONTENT_BLOCK_LIMIT
            || detail.bytes_read as usize >= CONTENT_TEXT_LIMIT
        {
            detail.truncated = true;
            break;
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No log rows were found.".into(),
        });
    }
    Ok(detail)
}

fn content_from_paths(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "filesystem.readOnly".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    for root in item.paths.iter().map(Path::new) {
        let entries: Vec<PathBuf> = if root.is_dir() {
            WalkDir::new(root)
                .max_depth(3)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file() && !entry.path_is_symlink())
                .take(100)
                .map(|entry| entry.into_path())
                .collect()
        } else if root.is_file() {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        };
        if entries.is_empty() && root.is_dir() {
            detail.blocks.push(ContentBlock::Notice {
                text: format!("{} is empty.", root.display()),
            });
        }
        for path in entries {
            if detail.blocks.len() >= CONTENT_BLOCK_LIMIT {
                detail.truncated = true;
                break;
            }
            preview_file(&path, &mut detail)?;
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No renderable files were found. Binary cache entries are listed only when they have a supported preview format.".into(),
        });
    }
    Ok(detail)
}

fn thumbnail_from_paths(item: &CleanupItem) -> Result<Option<ItemThumbnail>, CleanerError> {
    let mut candidates = Vec::new();
    for root in item.paths.iter().map(Path::new) {
        if root.is_dir() {
            candidates.extend(
                WalkDir::new(root)
                    .max_depth(3)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_file() && !entry.path_is_symlink())
                    .take(100)
                    .map(|entry| entry.into_path()),
            );
        } else if root.is_file() {
            candidates.push(root.to_path_buf());
        }
    }
    candidates.sort();
    for path in candidates {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(mime) = image_mime(&extension) else {
            continue;
        };
        let metadata = fs::metadata(&path)?;
        if metadata.len() > CONTENT_IMAGE_LIMIT {
            continue;
        }
        let bytes = fs::read(&path)?;
        return Ok(Some(ItemThumbnail {
            item_id: item.id.clone(),
            title: path.to_string_lossy().into_owned(),
            data_url: format!("data:{mime};base64,{}", BASE64.encode(bytes)),
        }));
    }
    Ok(None)
}

fn image_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn preview_file(path: &Path, detail: &mut ItemContentDetail) -> Result<(), CleanerError> {
    let title = path.to_string_lossy().into_owned();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let metadata = fs::metadata(path)?;
    if let Some(mime) = image_mime(&extension) {
        if metadata.len() <= CONTENT_IMAGE_LIMIT {
            let bytes = fs::read(path)?;
            detail.bytes_read = detail.bytes_read.saturating_add(bytes.len() as u64);
            detail.blocks.push(ContentBlock::Image {
                title,
                data_url: format!("data:{mime};base64,{}", BASE64.encode(bytes)),
            });
        } else {
            detail.blocks.push(ContentBlock::Notice {
                text: format!("{title} is larger than the 5 MB preview limit."),
            });
            detail.truncated = true;
        }
        return Ok(());
    }
    if !matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "json"
            | "jsonl"
            | "log"
            | "csv"
            | "html"
            | "css"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "rs"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "svg"
    ) {
        detail.blocks.push(ContentBlock::Notice {
            text: format!(
                "{title} · {} bytes · binary preview unavailable",
                metadata.len()
            ),
        });
        return Ok(());
    }
    let remaining = CONTENT_TEXT_LIMIT.saturating_sub(detail.bytes_read as usize);
    if remaining == 0 {
        detail.truncated = true;
        return Ok(());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((remaining + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > remaining {
        bytes.truncate(remaining);
        detail.truncated = true;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    detail.bytes_read = detail.bytes_read.saturating_add(bytes.len() as u64);
    detail.blocks.push(ContentBlock::Text { title, text });
    Ok(())
}

struct AppServerClient {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    connection_warning: Option<String>,
}

impl AppServerClient {
    async fn connect(
        binary: &Path,
        home: &Path,
        bypass_socket: bool,
    ) -> Result<Self, CleanerError> {
        let socket = home.join("ipc/ipc.sock");
        if socket.exists() && !bypass_socket {
            match Self::connect_transport(binary, home, Some(&socket)).await {
                Ok(client) => return Ok(client),
                Err(proxy_error) => {
                    let mut client = Self::connect_transport(binary, home, None)
                        .await
                        .map_err(|stdio_error| {
                            CleanerError::Integration(format!(
                                "control socket failed ({proxy_error}); stdio fallback failed ({stdio_error})"
                            ))
                        })?;
                    client.connection_warning = Some(
                        "Codex control socket did not respond; CleanerX safely connected through a temporary local App Server instead".into(),
                    );
                    return Ok(client);
                }
            }
        }
        Self::connect_transport(binary, home, None).await
    }

    async fn connect_transport(
        binary: &Path,
        home: &Path,
        socket: Option<&Path>,
    ) -> Result<Self, CleanerError> {
        let mut command = Command::new(binary);
        command.arg("app-server");
        if let Some(socket) = socket {
            command.arg("proxy").arg("--sock").arg(socket);
        } else {
            command.arg("--stdio");
        }
        command
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            CleanerError::Integration(format!("failed to start App Server: {error}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CleanerError::Integration("missing App Server stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CleanerError::Integration("missing App Server stdout".into()))?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
            connection_warning: None,
        };
        client
            .request_with_timeout(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "cleanerx",
                        "title": "CleanerX",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": { "experimentalApi": true }
                }),
                StdDuration::from_secs(3),
            )
            .await?;
        client.notify("initialized", Value::Null).await?;
        Ok(client)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), CleanerError> {
        let message = json!({ "method": method, "params": params });
        self.stdin.write_all(message.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, CleanerError> {
        self.request_with_timeout(method, params, StdDuration::from_secs(12))
            .await
    }

    async fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: StdDuration,
    ) -> Result<Value, CleanerError> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "id": id, "method": method, "params": params });
        self.stdin.write_all(message.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        loop {
            let line = tokio::time::timeout(timeout, self.stdout.next_line())
                .await
                .map_err(|_| CleanerError::Integration(format!("{method} timed out")))??
                .ok_or_else(|| {
                    CleanerError::Integration(format!("App Server closed during {method}"))
                })?;
            let value: Value = serde_json::from_str(&line).map_err(|error| {
                CleanerError::Integration(format!("invalid App Server response: {error}"))
            })?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(CleanerError::Integration(format!(
                    "{method}: {}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown JSON-RPC error")
                )));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn supports_memory_reset(&mut self) -> bool {
        // Invalid params proves that the method is registered without mutating memory.
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "id": id, "method": "memory/reset", "params": {} });
        if self
            .stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .is_err()
        {
            return false;
        }
        loop {
            let Ok(Ok(Some(line))) =
                tokio::time::timeout(StdDuration::from_secs(4), self.stdout.next_line()).await
            else {
                return false;
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            let code = value
                .pointer("/error/code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let message = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return code != -32601 && !message.to_ascii_lowercase().contains("not found");
        }
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn scan_sessions_from_server(
    client: &mut AppServerClient,
) -> Result<Vec<SessionRecord>, CleanerError> {
    let mut sessions = Vec::new();
    let loaded = list_loaded(client).await.unwrap_or_default();
    for archived in [false, true] {
        let mut cursor: Option<String> = None;
        loop {
            let result = client
                .request(
                    "thread/list",
                    json!({
                        "archived": archived,
                        "cursor": cursor,
                        "limit": 100,
                        "sourceKinds": SOURCE_KINDS,
                        "sortKey": "updated_at",
                        "sortDirection": "desc"
                    }),
                )
                .await?;
            let page = result
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| CleanerError::Integration("thread/list omitted data".into()))?;
            for thread in page {
                if let Some(session) = parse_server_thread(thread, archived, &loaded) {
                    sessions.push(session);
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(sessions)
}

async fn list_loaded(client: &mut AppServerClient) -> Result<HashSet<String>, CleanerError> {
    let mut loaded = HashSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let result = client
            .request(
                "thread/loaded/list",
                json!({ "cursor": cursor, "limit": 200 }),
            )
            .await?;
        if let Some(ids) = result.get("data").and_then(Value::as_array) {
            loaded.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(loaded)
}

fn parse_server_thread(
    thread: &Value,
    archived: bool,
    loaded: &HashSet<String>,
) -> Option<SessionRecord> {
    let id = thread.get("id")?.as_str()?.to_owned();
    let status = if loaded.contains(&id) {
        "loaded".to_owned()
    } else {
        status_name(thread.get("status"))
    };
    Some(SessionRecord {
        name: thread
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Untitled session")
            .to_owned(),
        cwd: thread
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        path: thread
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source: source_name(thread.get("source")),
        archived,
        pinned: thread
            .get("isPinned")
            .or_else(|| thread.get("pinned"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status,
        created_at: timestamp(thread.get("createdAt")),
        updated_at: timestamp(thread.get("updatedAt")),
        size_bytes: 0,
        parent_thread_id: thread
            .get("parentThreadId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        descendant_ids: Vec::new(),
        id,
    })
}

fn scan_sessions_from_state_db(home: &Path) -> Result<Vec<SessionRecord>, CleanerError> {
    let database = latest_state_db(home)
        .ok_or_else(|| CleanerError::NotFound("validated Codex state database".into()))?;
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    require_columns(
        &connection,
        "threads",
        &[
            "id",
            "rollout_path",
            "created_at",
            "updated_at",
            "source",
            "cwd",
            "title",
            "archived",
        ],
    )?;
    let mut statement = connection.prepare(
        "SELECT id, rollout_path, created_at, updated_at, source, cwd, title, archived FROM threads",
    )?;
    let records = statement.query_map([], |row| {
        let title: String = row.get(6)?;
        Ok(SessionRecord {
            id: row.get(0)?,
            path: Some(row.get(1)?),
            created_at: DateTime::from_timestamp(row.get(2)?, 0),
            updated_at: DateTime::from_timestamp(row.get(3)?, 0),
            source: row.get(4)?,
            cwd: row.get(5)?,
            name: if title.trim().is_empty() {
                "Untitled session".into()
            } else {
                title
            },
            archived: row.get::<_, i64>(7)? != 0,
            pinned: false,
            status: "notLoaded".into(),
            size_bytes: 0,
            parent_thread_id: None,
            descendant_ids: vec![],
        })
    })?;
    records
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CleanerError::Integration(error.to_string()))
}

fn augment_pinned_and_parents(
    home: &Path,
    sessions: &mut [SessionRecord],
    warnings: &mut Vec<String>,
) {
    let Some(database) = latest_state_db(home) else {
        return;
    };
    let result = (|| -> Result<(), CleanerError> {
        let connection = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        require_columns(&connection, "threads", &["id", "is_pinned"])?;
        let pinned = {
            let mut statement = connection.prepare("SELECT id FROM threads WHERE is_pinned = 1")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<HashSet<_>, _>>()?
        };
        let parents = if require_columns(
            &connection,
            "thread_spawn_edges",
            &["parent_thread_id", "child_thread_id"],
        )
        .is_ok()
        {
            let mut statement = connection
                .prepare("SELECT parent_thread_id, child_thread_id FROM thread_spawn_edges")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, String>(0)?))
                })?
                .collect::<Result<HashMap<_, _>, _>>()?
        } else {
            HashMap::new()
        };
        for session in sessions {
            session.pinned = pinned.contains(&session.id);
            if session.parent_thread_id.is_none() {
                session.parent_thread_id = parents.get(&session.id).cloned();
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        warnings.push(format!(
            "Pinned/child metadata was ignored because its schema is not recognized: {error}"
        ));
    }
}

fn populate_descendants(sessions: &mut [SessionRecord]) {
    let parent_map: HashMap<String, Option<String>> = sessions
        .iter()
        .map(|session| (session.id.clone(), session.parent_thread_id.clone()))
        .collect();
    for session in sessions {
        session.descendant_ids = parent_map
            .keys()
            .filter(|candidate| is_descendant(candidate, &session.id, &parent_map))
            .cloned()
            .collect();
        session.descendant_ids.sort();
    }
}

fn is_descendant(
    candidate: &str,
    ancestor: &str,
    parents: &HashMap<String, Option<String>>,
) -> bool {
    let mut current = parents.get(candidate).and_then(Clone::clone);
    let mut visited = HashSet::new();
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        if !visited.insert(parent.clone()) {
            break;
        }
        current = parents.get(&parent).and_then(Clone::clone);
    }
    false
}

fn scan_memory(home: &Path, items: &mut Vec<CleanupItem>) -> Result<(), CleanerError> {
    let candidates = [
        home.join("memories"),
        home.join("memories_1.sqlite"),
        home.join("memories_1.sqlite-wal"),
        home.join("memories_1.sqlite-shm"),
        home.join("sqlite/memories_1.sqlite"),
        home.join("sqlite/memories_1.sqlite-wal"),
        home.join("sqlite/memories_1.sqlite-shm"),
    ];
    let paths: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    if paths.is_empty() {
        return Ok(());
    }
    let size_bytes = paths
        .iter()
        .map(|path| cleanerx_core::safety::allocated_size(path).unwrap_or(0))
        .sum();
    items.push(CleanupItem {
        id: "memory:global".into(),
        category: StorageCategory::Memory,
        title: "Global Codex memory".into(),
        subtitle: Some("Merged memory cannot be reliably separated by project".into()),
        paths: paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        project_id: None,
        thread_id: None,
        size_bytes,
        modified_at: newest_modified(&paths),
        risk: RiskLevel::High,
        recoverable: true,
        default_selected: false,
        protected: false,
        blocked_reason: None,
        metadata: BTreeMap::from([
            ("scope".into(), "global".into()),
            ("files".into(), paths.len().to_string()),
        ]),
    });
    Ok(())
}

fn scan_orphan_generated_content(
    home: &Path,
    sessions: &[SessionRecord],
    items: &mut Vec<CleanupItem>,
) -> Result<(), CleanerError> {
    let known: HashSet<&str> = sessions.iter().map(|session| session.id.as_str()).collect();
    for (directory, category, label) in [
        ("attachments", StorageCategory::Attachment, "Attachment"),
        (
            "generated_images",
            StorageCategory::GeneratedImage,
            "Generated image",
        ),
        (
            "visualizations",
            StorageCategory::GeneratedImage,
            "Generated visualization",
        ),
    ] {
        let root = home.join(directory);
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if known.contains(file_name.as_str()) {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            items.push(CleanupItem {
                id: format!("orphan:{directory}:{file_name}"),
                category,
                title: format!("{label} · {file_name}"),
                subtitle: Some("Not linked to a currently indexed session".into()),
                paths: vec![path.to_string_lossy().into_owned()],
                project_id: None,
                thread_id: None,
                size_bytes: cleanerx_core::safety::allocated_size(&path).unwrap_or(0),
                modified_at: modified_at(&path),
                risk: RiskLevel::Review,
                recoverable: true,
                default_selected: false,
                protected: false,
                blocked_reason: None,
                metadata: BTreeMap::from([
                    ("association".into(), "orphaned".into()),
                    (
                        "entryType".into(),
                        if metadata.is_dir() {
                            "directory"
                        } else {
                            "file"
                        }
                        .into(),
                    ),
                ]),
            });
        }
    }
    Ok(())
}

fn scan_logs(home: &Path, items: &mut Vec<CleanupItem>) -> Result<(), CleanerError> {
    let candidates = [
        home.join("logs_2.sqlite"),
        home.join("logs_2.sqlite-wal"),
        home.join("logs_2.sqlite-shm"),
        home.join("sqlite/logs_2.sqlite"),
        home.join("sqlite/logs_2.sqlite-wal"),
        home.join("sqlite/logs_2.sqlite-shm"),
    ];
    let paths: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    if !paths.is_empty() {
        items.push(CleanupItem {
            id: "logs:database".into(),
            category: StorageCategory::Log,
            title: "Codex diagnostic logs".into(),
            subtitle: Some("Only entries older than the retention period are removed".into()),
            size_bytes: paths
                .iter()
                .map(|path| cleanerx_core::safety::allocated_size(path).unwrap_or(0))
                .sum(),
            modified_at: newest_modified(&paths),
            paths: paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            project_id: None,
            thread_id: None,
            risk: RiskLevel::Safe,
            recoverable: false,
            default_selected: false,
            protected: false,
            blocked_reason: None,
            metadata: BTreeMap::from([("retentionDays".into(), "7".into())]),
        });
    }
    Ok(())
}

fn scan_caches(
    home: &Path,
    app_support: Option<&Path>,
    running: bool,
    items: &mut Vec<CleanupItem>,
) -> Result<(), CleanerError> {
    let mut candidates = vec![home.join("cache")];
    if let Some(app_support) = app_support {
        candidates.extend(
            [
                "Cache",
                "Code Cache",
                "GPUCache",
                "DawnGraphiteCache",
                "DawnWebGPUCache",
                "GraphiteDawnCache",
                "Crashpad/completed",
                "Crashpad/pending",
            ]
            .map(|suffix| app_support.join(suffix)),
        );
    }
    for (index, path) in candidates.into_iter().enumerate() {
        if !path.exists() || fs::symlink_metadata(&path)?.file_type().is_symlink() {
            continue;
        }
        items.push(CleanupItem {
            id: format!("cache:{index}"),
            category: StorageCategory::Cache,
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Cache".into()),
            subtitle: Some("Allowlisted, regenerable cache".into()),
            paths: vec![path.to_string_lossy().into_owned()],
            project_id: None,
            thread_id: None,
            size_bytes: cleanerx_core::safety::allocated_size(&path).unwrap_or(0),
            modified_at: modified_at(&path),
            risk: RiskLevel::Safe,
            recoverable: false,
            default_selected: false,
            protected: false,
            blocked_reason: running.then(|| "Quit Codex before clearing writable caches".into()),
            metadata: BTreeMap::from([
                ("regenerable".into(), "true".into()),
                ("requiresCodexExit".into(), running.to_string()),
            ]),
        });
    }
    Ok(())
}

fn scan_temporary(
    home: &Path,
    app_support: Option<&Path>,
    running: bool,
    items: &mut Vec<CleanupItem>,
) -> Result<(), CleanerError> {
    let cutoff = SystemTime::now()
        .checked_sub(StdDuration::from_secs(24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut roots = vec![home.to_path_buf()];
    if let Some(app_support) = app_support {
        roots.push(app_support.to_path_buf());
    }
    let mut index = 0;
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_temp = name.contains(".tmp-")
                || name.ends_with(".tmp")
                || name.ends_with(".partial")
                || name == "tmp";
            if !is_temp {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink()
                || metadata
                    .modified()
                    .map(|time| time >= cutoff)
                    .unwrap_or(true)
            {
                continue;
            }
            items.push(CleanupItem {
                id: format!("temporary:{index}"),
                category: StorageCategory::Temporary,
                title: name,
                subtitle: Some("Temporary data older than 24 hours".into()),
                paths: vec![path.to_string_lossy().into_owned()],
                project_id: None,
                thread_id: None,
                size_bytes: cleanerx_core::safety::allocated_size(&path).unwrap_or(0),
                modified_at: modified_at(&path),
                risk: RiskLevel::Safe,
                recoverable: false,
                default_selected: false,
                protected: false,
                blocked_reason: running.then(|| "Quit Codex before clearing temporary data".into()),
                metadata: BTreeMap::from([
                    ("olderThanHours".into(), "24".into()),
                    ("requiresCodexExit".into(), running.to_string()),
                ]),
            });
            index += 1;
        }
    }
    Ok(())
}

fn scan_protected(home: &Path, items: &mut Vec<CleanupItem>) -> Result<(), CleanerError> {
    for name in PROTECTED_NAMES {
        let path = home.join(name);
        if !path.exists() {
            continue;
        }
        items.push(CleanupItem {
            id: format!("protected:{name}"),
            category: StorageCategory::Protected,
            title: (*name).to_owned(),
            subtitle: None,
            paths: vec![path.to_string_lossy().into_owned()],
            project_id: None,
            thread_id: None,
            size_bytes: cleanerx_core::safety::allocated_size(&path).unwrap_or(0),
            modified_at: modified_at(&path),
            risk: RiskLevel::Protected,
            recoverable: false,
            default_selected: false,
            protected: true,
            blocked_reason: Some("Protected data".into()),
            metadata: BTreeMap::from([("protection".into(), "always".into())]),
        });
    }
    Ok(())
}

fn group_projects(
    sessions: &[SessionRecord],
    home: &Path,
    warnings: &mut Vec<String>,
) -> Vec<ProjectGroup> {
    let database_projects = read_database_projects(home).unwrap_or_else(|error| {
        warnings.push(format!(
            "Project registry metadata was ignored because its schema is not recognized: {error}"
        ));
        HashMap::new()
    });
    let mut groups: BTreeMap<String, ProjectGroup> = BTreeMap::new();
    for session in sessions {
        let root = project_root(&session.cwd);
        let id = project_id_for_cwd(&root);
        let database_name = database_projects.get(&root).cloned();
        let name = database_name.unwrap_or_else(|| {
            Path::new(&root)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| root.clone())
        });
        let group = groups.entry(id.clone()).or_insert(ProjectGroup {
            id,
            name,
            roots: vec![root.clone()],
            session_ids: vec![],
            size_bytes: 0,
        });
        group.session_ids.push(session.id.clone());
        group.size_bytes = group.size_bytes.saturating_add(session.size_bytes);
    }
    groups.into_values().collect()
}

fn read_database_projects(home: &Path) -> Result<HashMap<String, String>, CleanerError> {
    let Some(database) = latest_state_db(home) else {
        return Ok(HashMap::new());
    };
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    require_columns(&connection, "projects", &["id", "name"])?;
    require_columns(&connection, "project_roots", &["project_id", "path"])?;
    let mut statement = connection.prepare(
        "SELECT project_roots.path, projects.name FROM project_roots JOIN projects ON projects.id = project_roots.project_id",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?)
}

fn summarize_categories(items: &[CleanupItem]) -> Vec<CategorySummary> {
    let mut summaries: BTreeMap<StorageCategory, CategorySummary> = BTreeMap::new();
    for item in items {
        let summary = summaries.entry(item.category).or_insert(CategorySummary {
            category: item.category,
            size_bytes: 0,
            item_count: 0,
            default_selected_bytes: 0,
        });
        summary.size_bytes = summary.size_bytes.saturating_add(item.size_bytes);
        summary.item_count += 1;
        if item.default_selected {
            summary.default_selected_bytes = summary
                .default_selected_bytes
                .saturating_add(item.size_bytes);
        }
    }
    summaries.into_values().collect()
}

fn associated_paths(home: &Path, session_id: &str) -> Vec<PathBuf> {
    [
        home.join("attachments").join(session_id),
        home.join("generated_images").join(session_id),
        home.join("visualizations").join(session_id),
        home.join("shell_snapshots").join(session_id),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn latest_state_db(home: &Path) -> Option<PathBuf> {
    let roots = [home.to_path_buf(), home.join("sqlite")];
    let mut candidates = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("state_") && name.ends_with(".sqlite") {
                candidates.push(entry.path());
            }
        }
    }
    candidates.sort_by_key(|path| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("state_"))
            .and_then(|version| version.parse::<u32>().ok())
            .unwrap_or_default()
    });
    candidates.pop()
}

fn require_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<(), CleanerError> {
    if !table
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(CleanerError::Integration("invalid table identifier".into()));
    }
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if required.iter().any(|column| !columns.contains(*column)) {
        return Err(CleanerError::Integration(format!(
            "unrecognized {table} schema"
        )));
    }
    Ok(())
}

fn find_codex_binary() -> Option<PathBuf> {
    let executable_name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            candidates.push(directory.join(executable_name));
        }
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ]);
    if let Some(home) = dirs::home_dir() {
        candidates.extend([
            home.join(".local/bin").join(executable_name),
            home.join(".volta/bin").join(executable_name),
            home.join(".asdf/shims").join(executable_name),
            home.join(".bun/bin").join(executable_name),
            home.join("Library/pnpm").join(executable_name),
        ]);
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut nvm_candidates: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin").join(executable_name))
                .filter(|path| path.is_file())
                .collect();
            nvm_candidates.sort_by_key(|path| {
                std::cmp::Reverse(
                    fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                )
            });
            candidates.extend(nvm_candidates);
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn codex_app_support() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        candidates.extend([
            home.join("Library/Application Support/com.openai.codex"),
            home.join("Library/Application Support/Codex"),
            home.join("Library/Application Support/OpenAI/Codex"),
        ]);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(data) = dirs::data_dir() {
        candidates.extend([data.join("Codex"), data.join("com.openai.codex")]);
    }
    candidates.into_iter().find(|path| path.is_dir())
}

fn codex_is_running(home: &Path) -> bool {
    if home.join("ipc/ipc.sock").exists() {
        return true;
    }
    let system = System::new_all();
    system.processes().values().any(|process| {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        let command = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        name == "codex" || name.contains("codex app") || command.contains("codex app-server")
    })
}

fn project_root(cwd: &str) -> String {
    let path = Path::new(cwd);
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for ancestor in normalized.ancestors() {
        if ancestor.join(".git").exists() {
            return ancestor.to_string_lossy().into_owned();
        }
    }
    normalized.to_string_lossy().into_owned()
}

fn project_id_for_cwd(cwd: &str) -> String {
    let root = project_root(cwd);
    Uuid::new_v5(&Uuid::NAMESPACE_URL, root.as_bytes()).to_string()
}

fn status_name(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(status)) => status.clone(),
        Some(Value::Object(status)) => status
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| status.keys().next().cloned())
            .unwrap_or_else(|| "unknown".into()),
        _ => "unknown".into(),
    }
}

fn source_name(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(source)) => source.clone(),
        Some(Value::Object(source)) => source
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown".into()),
        _ => "unknown".into(),
    }
}

fn timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_i64)
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

fn modified_at(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn newest_modified(paths: &[PathBuf]) -> Option<DateTime<Utc>> {
    paths.iter().filter_map(|path| modified_at(path)).max()
}

fn is_active_status(status: &str) -> bool {
    matches!(status.to_ascii_lowercase().as_str(), "active" | "loaded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_precedence_prefers_override() {
        let adapter = CodexAdapter::new();
        let temp = tempfile::tempdir().expect("temp");
        assert_eq!(
            adapter
                .resolve_home(Some(temp.path().to_str().expect("path")))
                .expect("home"),
            temp.path()
        );
    }

    #[test]
    fn descendants_include_all_generations() {
        let mut records = vec![
            record("root", None),
            record("child", Some("root")),
            record("grandchild", Some("child")),
        ];
        populate_descendants(&mut records);
        assert_eq!(records[0].descendant_ids, vec!["child", "grandchild"]);
    }

    #[test]
    fn thread_read_content_is_rendered_without_raw_protocol_objects() {
        let item = cleanup_item("session:test", StorageCategory::Session, Vec::new());
        let result = json!({
            "thread": {
                "turns": [{
                    "items": [
                        { "type": "userMessage", "content": [{ "type": "text", "text": "hello" }] },
                        { "type": "agentMessage", "text": "hi", "phase": "final_answer" },
                        { "type": "commandExecution", "command": ["cargo", "test"], "aggregatedOutput": "ok" }
                    ]
                }]
            }
        });

        let detail = content_from_thread_read(&item, &result);

        assert_eq!(detail.source, "appServer.thread/read");
        assert_eq!(detail.blocks.len(), 3);
        assert!(matches!(
            &detail.blocks[0],
            ContentBlock::Message { role, text, .. } if role == "user" && text == "hello"
        ));
        assert!(matches!(
            &detail.blocks[1],
            ContentBlock::Message { role, text, .. } if role == "assistant" && text == "hi"
        ));
        assert!(matches!(
            &detail.blocks[2],
            ContentBlock::Text { title, text } if title == "Command" && text.contains("cargo test")
        ));
    }

    #[test]
    fn recognized_memory_schema_is_loaded_read_only() {
        let temp = tempfile::tempdir().expect("temp");
        let database = temp.path().join("memories_1.sqlite");
        let connection = Connection::open(&database).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE stage1_outputs (
                    thread_id TEXT PRIMARY KEY,
                    source_updated_at INTEGER NOT NULL,
                    raw_memory TEXT NOT NULL,
                    rollout_summary TEXT NOT NULL,
                    rollout_slug TEXT,
                    generated_at INTEGER NOT NULL
                );
                INSERT INTO stage1_outputs VALUES ('thread-1', 1, 'Remember the tree view', 'CleanerX preferences', 'CleanerX', 1);",
            )
            .expect("fixture");
        drop(connection);
        let item = cleanup_item(
            "memory:global",
            StorageCategory::Memory,
            vec![database.to_string_lossy().into_owned()],
        );

        let detail = content_from_memory(&item).expect("content");

        assert!(matches!(
            &detail.blocks[0],
            ContentBlock::Text { title, text }
                if title.contains("CleanerX") && text.contains("Remember the tree view")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn content_path_validation_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp");
        let target = temp.path().join("target.txt");
        fs::write(&target, "content").expect("target");
        let link = temp.path().join("preview.txt");
        symlink(&target, &link).expect("symlink");
        let item = cleanup_item(
            "attachment:test",
            StorageCategory::Attachment,
            vec![link.to_string_lossy().into_owned()],
        );
        let installation = AgentInstallation {
            kind: AgentKind::Codex,
            home: temp.path().to_string_lossy().into_owned(),
            binary: None,
            version: None,
            app_support: None,
            running: false,
            capabilities: AgentCapabilities::default(),
            warnings: Vec::new(),
        };

        assert!(matches!(
            validate_content_paths(&installation, &item),
            Err(CleanerError::UnsafePath(_))
        ));
    }

    #[test]
    fn media_thumbnail_reads_only_a_bounded_supported_image() {
        let temp = tempfile::tempdir().expect("temp");
        let image = temp.path().join("preview.png");
        let bytes = BASE64
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .expect("png fixture");
        fs::write(&image, bytes).expect("image");
        fs::write(temp.path().join("private.txt"), "not a thumbnail").expect("text");
        let item = cleanup_item(
            "attachment:test",
            StorageCategory::Attachment,
            vec![temp.path().to_string_lossy().into_owned()],
        );

        let thumbnail = thumbnail_from_paths(&item)
            .expect("thumbnail")
            .expect("supported image");

        assert_eq!(thumbnail.item_id, item.id);
        assert_eq!(thumbnail.title, image.to_string_lossy());
        assert!(thumbnail.data_url.starts_with("data:image/png;base64,"));
    }

    #[tokio::test]
    #[ignore = "requires a local Codex installation"]
    async fn live_detects_official_write_capabilities() {
        let adapter = CodexAdapter::new();
        let installation = adapter.detect(None).await.expect("detect local Codex");
        assert!(installation.capabilities.thread_list);
        assert!(installation.capabilities.thread_delete);
        assert!(!installation.capabilities.report_only);
        let snapshot = adapter.scan(None).await.expect("scan local Codex");
        assert!(snapshot.installation.capabilities.thread_list);
        assert!(snapshot.installation.capabilities.thread_delete);
        assert!(!snapshot.installation.capabilities.report_only);
    }

    fn record(id: &str, parent: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            name: id.into(),
            cwd: "/tmp/project".into(),
            path: None,
            source: "cli".into(),
            archived: false,
            pinned: false,
            status: "notLoaded".into(),
            created_at: None,
            updated_at: None,
            size_bytes: 0,
            parent_thread_id: parent.map(str::to_owned),
            descendant_ids: vec![],
        }
    }

    fn cleanup_item(id: &str, category: StorageCategory, paths: Vec<String>) -> CleanupItem {
        CleanupItem {
            id: id.into(),
            category,
            title: id.into(),
            subtitle: None,
            paths,
            project_id: None,
            thread_id: (category == StorageCategory::Session).then(|| "test".into()),
            size_bytes: 0,
            modified_at: None,
            risk: RiskLevel::Review,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason: None,
            metadata: BTreeMap::new(),
        }
    }
}
