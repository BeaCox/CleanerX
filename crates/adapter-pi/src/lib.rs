//! pi coding-agent storage discovery and bounded, local-only content access.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentCapabilities, AgentDetectionState, AgentInstallation, AgentKind,
    CategorySummary, CleanerError, CleanupItem, ContentBlock, InventorySnapshot, ItemContentDetail,
    ItemThumbnail, MemoryCapabilities, ProjectGroup, RiskLevel, SessionRecord, StorageCategory,
};
use serde_json::Value;
use sysinfo::System;
use tokio::process::Command;
use uuid::Uuid;

const METADATA_SCAN_LIMIT: u64 = 2 * 1024 * 1024;
const SESSION_TITLE_CHAR_LIMIT: usize = 96;
const CONTENT_TEXT_LIMIT: usize = 512 * 1024;
const CONTENT_BLOCK_LIMIT: usize = 200;
const TOOL_TEXT_LIMIT: usize = 16 * 1024;

const PROTECTED_NAMES: &[&str] = &[
    "auth.json",
    "settings.json",
    "models.json",
    "trust.json",
    "keybindings.json",
    "AGENTS.md",
    "SYSTEM.md",
    "APPEND_SYSTEM.md",
    "prompts",
    "skills",
    "extensions",
    "themes",
    "git",
    "npm",
];

#[derive(Debug, Clone, Default)]
pub struct PiAdapter;

impl PiAdapter {
    pub fn new() -> Self {
        Self
    }

    fn resolve_home(&self, custom_home: Option<&str>) -> Result<PathBuf, CleanerError> {
        let path = custom_home
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("PI_CODING_AGENT_DIR").map(PathBuf::from))
            .or_else(|| dirs::home_dir().map(|home| home.join(".pi").join("agent")))
            .ok_or_else(|| CleanerError::NotFound("pi agent directory".into()))?;
        if !path.is_absolute() {
            return Err(CleanerError::InvalidRequest(
                "pi agent directory override must be an absolute path".into(),
            ));
        }
        Ok(path)
    }
}

#[async_trait]
impl AgentAdapter for PiAdapter {
    async fn detect(&self, custom_home: Option<&str>) -> Result<AgentInstallation, CleanerError> {
        let home = self.resolve_home(custom_home)?;
        let binary = find_pi_binary();
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
        let running = pi_is_running();
        let mut warnings = Vec::new();
        if !home.exists() {
            warnings.push(format!(
                "pi agent directory does not exist: {}",
                home.display()
            ));
        }
        if binary.is_none() {
            warnings.push(
                "pi executable was not found; its installed version cannot be reported".into(),
            );
        }

        let recognized_storage = home.is_dir();
        Ok(AgentInstallation {
            kind: AgentKind::Pi,
            state: AgentDetectionState::from_presence(binary.is_some(), recognized_storage),
            home: home.to_string_lossy().into_owned(),
            binary: binary.map(|path| path.to_string_lossy().into_owned()),
            version,
            app_support: None,
            running,
            capabilities: AgentCapabilities {
                thread_list: recognized_storage,
                thread_delete: recognized_storage,
                memory: MemoryCapabilities::default(),
                descendant_filter: false,
                report_only: !recognized_storage,
            },
            warnings,
        })
    }

    async fn scan(&self, custom_home: Option<&str>) -> Result<InventorySnapshot, CleanerError> {
        let installation = self.detect(custom_home).await?;
        let home = PathBuf::from(&installation.home);
        let mut warnings = installation.warnings.clone();
        let mut sessions = Vec::new();
        let mut items = Vec::new();
        let mut projects = Vec::new();

        scan_sessions(
            &home,
            installation.running,
            &mut sessions,
            &mut items,
            &mut projects,
            &mut warnings,
        )?;
        scan_application_data(&home, installation.running, &mut items, &mut warnings)?;
        scan_protected(&home, &mut items, &mut warnings);

        let categories = summarize_categories(&items);
        let total_bytes = items
            .iter()
            .filter(|item| item.category != StorageCategory::Protected)
            .map(|item| item.size_bytes)
            .sum();

        Ok(InventorySnapshot {
            id: Uuid::new_v4(),
            scanned_at: Utc::now(),
            installation,
            total_bytes,
            reclaimable_bytes: 0,
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
                "CleanerX never opens pi authentication, settings, trust decisions, rules, skills, extensions, or installed packages.",
            ));
        }
        match item.category {
            StorageCategory::Session | StorageCategory::ArchivedSession => {
                content_from_session(item)
            }
            _ => Ok(content_notice(
                item,
                "filesystem.metadataOnly",
                "This recognized pi application-data item is inventoried by metadata only.",
            )),
        }
    }

    async fn load_item_thumbnail(
        &self,
        _installation: &AgentInstallation,
        _item: &CleanupItem,
    ) -> Result<Option<ItemThumbnail>, CleanerError> {
        Err(CleanerError::Unsupported(
            "pi inventory does not expose thumbnail items".into(),
        ))
    }

    async fn delete_sessions(
        &self,
        _installation: &AgentInstallation,
        _session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError> {
        Err(CleanerError::Unsupported(
            "pi session removal requires CleanerX's preflighted path transaction".into(),
        ))
    }

    async fn reset_memory(&self, _installation: &AgentInstallation) -> Result<(), CleanerError> {
        Err(CleanerError::Unsupported(
            "pi does not expose a supported automatic-memory capability".into(),
        ))
    }
}

#[derive(Debug, Default)]
struct SessionFileMetadata {
    id: String,
    cwd: String,
    created_at: Option<DateTime<Utc>>,
    name: Option<String>,
    first_user_message: Option<String>,
    parent_session: Option<PathBuf>,
}

/// One recognized `<bucket>/<timestamp>_<uuid>.jsonl` session file.
#[derive(Debug)]
struct DiscoveredSession {
    metadata: SessionFileMetadata,
    /// The path exactly as discovered beneath the pi agent directory.
    path: PathBuf,
    /// The canonicalized path used only for parent-session matching.
    canonical: PathBuf,
    bucket: PathBuf,
    size_bytes: u64,
    updated_at: Option<DateTime<Utc>>,
}

#[allow(clippy::too_many_arguments)]
fn scan_sessions(
    home: &Path,
    running: bool,
    sessions: &mut Vec<SessionRecord>,
    items: &mut Vec<CleanupItem>,
    projects: &mut Vec<ProjectGroup>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let sessions_root = home.join("sessions");
    if !sessions_root.exists() {
        return Ok(());
    }
    let mut discovered = Vec::new();
    let mut seen_ids = HashSet::new();
    let Ok(buckets) = fs::read_dir(&sessions_root) else {
        return Ok(());
    };
    for bucket in buckets.flatten() {
        let bucket_path = bucket.path();
        if !is_plain_directory(&bucket_path)? {
            if bucket_path.symlink_metadata().is_ok() {
                warnings.push(format!(
                    "Skipped linked or unrecognized pi session bucket: {}",
                    bucket_path.display()
                ));
            }
            continue;
        }
        let Ok(entries) = fs::read_dir(&bucket_path) else {
            continue;
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            if !is_plain_file(&path)? {
                warnings.push(format!(
                    "Skipped linked or unrecognized pi session file: {}",
                    path.display()
                ));
                continue;
            }
            files.push(path);
        }
        files.sort();
        for path in files {
            let metadata = match read_session_metadata(&path) {
                Ok(Some(metadata)) => metadata,
                Ok(None) => {
                    warnings.push(format!(
                        "Skipped pi session file with an unrecognized header: {}",
                        path.display()
                    ));
                    continue;
                }
                Err(error) => {
                    warnings.push(format!(
                        "Skipped unreadable pi session file {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            if !seen_ids.insert(metadata.id.clone()) {
                warnings.push(format!(
                    "Skipped duplicate pi session ID {} in {}",
                    metadata.id,
                    path.display()
                ));
                continue;
            }
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            let canonical_bucket = bucket_path
                .canonicalize()
                .unwrap_or_else(|_| bucket_path.clone());
            let size_bytes = cleanerx_core::safety::allocated_size(&path)?;
            let updated_at = modified_at(&path);
            discovered.push(DiscoveredSession {
                metadata,
                path,
                canonical,
                bucket: canonical_bucket,
                size_bytes,
                updated_at,
            });
        }
    }

    let id_by_path: HashMap<&Path, &str> = discovered
        .iter()
        .map(|session| (session.canonical.as_path(), session.metadata.id.as_str()))
        .collect();
    let mut parent_links: Vec<(usize, String)> = Vec::new();
    for (index, session) in discovered.iter().enumerate() {
        let Some(parent_path) = &session.metadata.parent_session else {
            continue;
        };
        let parent_id = parent_path
            .canonicalize()
            .ok()
            .filter(|canonical| {
                canonical.starts_with(
                    sessions_root
                        .canonicalize()
                        .unwrap_or(sessions_root.clone()),
                )
            })
            .as_deref()
            .and_then(|canonical| id_by_path.get(canonical).copied());
        let Some(parent_id) = parent_id else {
            continue;
        };
        if parent_id != session.metadata.id {
            parent_links.push((index, parent_id.to_owned()));
        }
    }
    let parent_by_id: HashMap<&str, &str> = parent_links
        .iter()
        .map(|(index, parent)| (discovered[*index].metadata.id.as_str(), parent.as_str()))
        .collect();
    let mut parent_thread_ids = HashMap::<usize, String>::new();
    for (index, parent_id) in &parent_links {
        let session_id = &discovered[*index].metadata.id;
        let mut cycle = false;
        let mut cursor = parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(next) = parent_by_id.get(cursor.as_str()) {
            if *next == session_id.as_str() || !visited.insert(cursor.clone()) {
                cycle = true;
                break;
            }
            cursor = (*next).to_owned();
        }
        if !cycle {
            parent_thread_ids.insert(*index, parent_id.clone());
        }
    }

    let mut project_buckets: BTreeMap<PathBuf, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    for (index, session) in discovered.iter().enumerate() {
        project_buckets
            .entry(session.bucket.clone())
            .or_default()
            .entry(session.metadata.cwd.clone())
            .or_default()
            .push(index);
    }
    let mut project_id_by_bucket: HashMap<PathBuf, String> = HashMap::new();
    for (bucket, cwd_groups) in &project_buckets {
        let project_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, bucket.to_string_lossy().as_bytes());
        project_id_by_bucket.insert(bucket.clone(), project_id.to_string());
        let mut group = ProjectGroup {
            id: project_id.to_string(),
            name: "pi project".into(),
            roots: Vec::new(),
            session_ids: Vec::new(),
            size_bytes: 0,
        };
        let mut named = false;
        for (cwd, indexes) in cwd_groups {
            let root = cwd.trim();
            if Path::new(root).is_absolute() {
                if !group.roots.iter().any(|known| known == root) {
                    group.roots.push(root.to_owned());
                }
                if !named {
                    group.name = project_name(root, bucket);
                    named = true;
                }
            }
            for index in indexes {
                group
                    .session_ids
                    .push(discovered[*index].metadata.id.clone());
                group.size_bytes = group
                    .size_bytes
                    .saturating_add(discovered[*index].size_bytes);
            }
        }
        projects.push(group);
    }

    for (index, session) in discovered.iter().enumerate() {
        let (source_revision, unsafe_reason) =
            source_revision(std::slice::from_ref(&session.path), warnings);
        let blocked_reason = running
            .then(|| "pi is running; quit it before deleting local session data".to_owned())
            .or(unsafe_reason);
        let name = session_display_name(&session.metadata);
        let record = SessionRecord {
            id: session.metadata.id.clone(),
            name: name.clone(),
            cwd: session.metadata.cwd.clone(),
            path: Some(session.path.to_string_lossy().into_owned()),
            source: "cli".into(),
            archived: false,
            pinned: false,
            status: "notLoaded".into(),
            created_at: session.metadata.created_at,
            updated_at: session.updated_at,
            size_bytes: session.size_bytes,
            parent_thread_id: parent_thread_ids.get(&index).cloned(),
            descendant_ids: Vec::new(),
        };
        let project_id = project_id_by_bucket
            .get(&session.bucket)
            .map(String::to_owned);
        items.push(CleanupItem {
            id: format!("session:{}", session.metadata.id),
            category: StorageCategory::Session,
            title: name,
            subtitle: (!session.metadata.cwd.is_empty()).then(|| session.metadata.cwd.clone()),
            paths: vec![session.path.to_string_lossy().into_owned()],
            project_id,
            thread_id: Some(session.metadata.id.clone()),
            size_bytes: session.size_bytes,
            modified_at: session.updated_at,
            risk: RiskLevel::High,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason,
            metadata: BTreeMap::from([
                ("source".into(), "cli".into()),
                ("pinned".into(), "false".into()),
                ("requiresAgentExit".into(), "true".into()),
            ])
            .into_iter()
            .chain(source_revision.map(|revision| ("sourceRevision".into(), revision)))
            .collect(),
        });
        sessions.push(record);
    }
    Ok(())
}

fn session_display_name(metadata: &SessionFileMetadata) -> String {
    if let Some(name) = metadata
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    if let Some(first_user_message) = metadata.first_user_message.as_deref() {
        return first_user_message.to_owned();
    }

    let short_id = &metadata.id[..metadata.id.len().min(8)];
    metadata
        .created_at
        .map(|created_at| {
            format!(
                "pi · {} UTC · {short_id}",
                created_at.format("%Y-%m-%d %H:%M")
            )
        })
        .unwrap_or_else(|| format!("pi · {short_id}"))
}

/// Reads the session header, latest `session_info` display name, and one bounded first-user
/// message title within a bounded prefix of the JSONL file. No other message or tool body is kept.
fn read_session_metadata(path: &Path) -> Result<Option<SessionFileMetadata>, CleanerError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file).take(METADATA_SCAN_LIMIT);
    let mut lines = reader.lines();
    let Some(header_line) = lines.next().transpose()? else {
        return Ok(None);
    };
    let header: Value = serde_json::from_str(header_line.trim()).map_err(CleanerError::Json)?;
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return Ok(None);
    }
    let Some(id) = header.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };
    if Uuid::parse_str(id).is_err() {
        return Ok(None);
    }
    let mut metadata = SessionFileMetadata {
        id: id.to_owned(),
        cwd: header
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|cwd| Path::new(cwd).is_absolute())
            .unwrap_or_default()
            .to_owned(),
        created_at: header
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_timestamp),
        name: None,
        first_user_message: None,
        parent_session: header
            .get("parentSession")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from),
    };
    for line in lines {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if !line.contains("\"session_info\"") && !line.contains("\"message\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("session_info") => {
                metadata.name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);
            }
            Some("message") if metadata.first_user_message.is_none() => {
                let Some(message) = entry.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) == Some("user") {
                    metadata.first_user_message =
                        normalized_session_title(&message_text(message.get("content")));
                }
            }
            _ => {}
        }
    }
    if metadata.name.is_some() {
        metadata.first_user_message = None;
    }
    Ok(Some(metadata))
}

fn normalized_session_title(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() <= SESSION_TITLE_CHAR_LIMIT {
        return Some(normalized);
    }
    let mut title = normalized
        .chars()
        .take(SESSION_TITLE_CHAR_LIMIT)
        .collect::<String>();
    title.push('…');
    Some(title)
}

fn scan_application_data(
    home: &Path,
    running: bool,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let cache = home.join("models-store.json");
    if !cache.exists() {
        return Ok(());
    }
    if !is_plain_file(&cache)? {
        warnings.push(format!(
            "Skipped linked or unrecognized pi model catalog cache: {}",
            cache.display()
        ));
        return Ok(());
    }
    let size_bytes = cleanerx_core::safety::allocated_size(&cache)?;
    let (source_revision, unsafe_reason) = source_revision(std::slice::from_ref(&cache), warnings);
    items.push(CleanupItem {
        id: "cache:pi-models-store".into(),
        category: StorageCategory::Cache,
        title: "pi model catalog cache".into(),
        subtitle: Some("Regenerable remote provider catalogs cached for offline use".into()),
        paths: vec![cache.to_string_lossy().into_owned()],
        project_id: None,
        thread_id: None,
        size_bytes,
        modified_at: modified_at(&cache),
        risk: RiskLevel::Safe,
        recoverable: false,
        default_selected: false,
        protected: false,
        blocked_reason: running
            .then(|| "pi is running; quit it before cleaning writable application data".into())
            .or(unsafe_reason),
        metadata: BTreeMap::from([
            ("requiresAgentExit".into(), "true".into()),
            ("regenerable".into(), "true".into()),
        ])
        .into_iter()
        .chain(source_revision.map(|revision| ("sourceRevision".into(), revision)))
        .collect(),
    });
    Ok(())
}

fn scan_protected(home: &Path, items: &mut Vec<CleanupItem>, warnings: &mut Vec<String>) {
    for name in PROTECTED_NAMES {
        let path = home.join(name);
        if !path.exists() {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warnings.push(format!(
                    "Could not inspect protected pi path {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        let size_bytes = if metadata.file_type().is_symlink() {
            warnings.push(format!(
                "Protected pi path is a symbolic link and was not followed: {}",
                path.display()
            ));
            0
        } else {
            cleanerx_core::safety::allocated_size(&path).unwrap_or_default()
        };
        items.push(CleanupItem {
            id: format!("protected:pi:{name}"),
            category: StorageCategory::Protected,
            title: (*name).into(),
            subtitle: Some(
                "pi authentication, configuration, trust, rules, skills, extensions, or packages"
                    .into(),
            ),
            paths: vec![path.to_string_lossy().into_owned()],
            project_id: None,
            thread_id: None,
            size_bytes,
            modified_at: modified_at(&path),
            risk: RiskLevel::Protected,
            recoverable: false,
            default_selected: false,
            protected: true,
            blocked_reason: Some("Protected pi data".into()),
            metadata: BTreeMap::from([("protection".into(), "always".into())]),
        });
    }
}

fn validate_content_paths(
    installation: &AgentInstallation,
    item: &CleanupItem,
) -> Result<(), CleanerError> {
    if installation.kind != AgentKind::Pi {
        return Err(CleanerError::InvalidRequest(
            "pi content request used a different Agent installation".into(),
        ));
    }
    let root = PathBuf::from(&installation.home).canonicalize()?;
    for raw_path in &item.paths {
        let path = Path::new(raw_path);
        if !path.exists() {
            continue;
        }
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(CleanerError::UnsafePath(raw_path.clone()));
        }
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(CleanerError::UnsafePath(raw_path.clone()));
        }
    }
    Ok(())
}

fn content_from_session(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let path = item
        .paths
        .iter()
        .map(Path::new)
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .ok_or_else(|| CleanerError::NotFound("pi session file".into()))?;
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file).take(CONTENT_TEXT_LIMIT as u64);
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "piSession.readOnly".into(),
        truncated: fs::metadata(path)?.len() > CONTENT_TEXT_LIMIT as u64,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        detail.bytes_read = detail.bytes_read.saturating_add(line.len() as u64);
        if detail.blocks.len() >= CONTENT_BLOCK_LIMIT
            || detail.bytes_read >= CONTENT_TEXT_LIMIT as u64
        {
            detail.truncated = true;
            break;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                let text = message_text(message.get("content"));
                if !text.trim().is_empty() {
                    detail.blocks.push(ContentBlock::Message {
                        role: "user".into(),
                        text: bounded_string(text, CONTENT_TEXT_LIMIT / 2),
                        phase: None,
                    });
                }
            }
            Some("assistant") => {
                let text = message_text(message.get("content"));
                if !text.trim().is_empty() {
                    detail.blocks.push(ContentBlock::Message {
                        role: "assistant".into(),
                        text: bounded_string(text, CONTENT_TEXT_LIMIT / 2),
                        phase: None,
                    });
                }
            }
            Some("toolResult") => {
                let title = message
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("Tool")
                    .to_owned();
                let text = message_text(message.get("content"));
                if !text.trim().is_empty() {
                    detail.blocks.push(ContentBlock::Text {
                        title: bounded_string(title, 256),
                        text: bounded_string(text, TOOL_TEXT_LIMIT),
                    });
                }
            }
            Some("bashExecution") => {
                let command = message
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let output = message
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                detail.blocks.push(ContentBlock::Text {
                    title: "bash".into(),
                    text: bounded_string(format!("{command}\n{output}"), TOOL_TEXT_LIMIT),
                });
            }
            _ => {}
        }
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No supported message blocks were found in this pi session preview.".into(),
        });
    }
    Ok(detail)
}

fn message_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_object()
                    .filter(|object| object.get("type").and_then(Value::as_str) == Some("text"))
                    .and_then(|object| object.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
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

fn source_revision(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
) -> (Option<String>, Option<String>) {
    match cleanerx_core::metadata_revision(paths) {
        Ok(revision) => (Some(revision), None),
        Err(error) => {
            warnings.push(format!(
                "pi data was made read-only by a safety check: {error}"
            ));
            (
                None,
                Some("Linked, foreign-owned, or unstable pi data cannot be cleaned".into()),
            )
        }
    }
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
    }
    summaries.into_values().collect()
}

fn project_name(root: &str, bucket: &Path) -> String {
    if root == "/" {
        return "Global".into();
    }
    Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            bucket
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "pi project".into())
}

fn parse_rfc3339_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn is_plain_directory(path: &Path) -> Result<bool, CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn is_plain_file(path: &Path) -> Result<bool, CleanerError> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_file() && !metadata.file_type().is_symlink())
}

fn modified_at(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn bounded_string(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let boundary = (0..=limit)
        .rev()
        .find(|index| value.is_char_boundary(*index))
        .unwrap_or_default();
    value.truncate(boundary);
    value.push_str("\n…");
    value
}

fn find_pi_binary() -> Option<PathBuf> {
    let executable_name = if cfg!(windows) { "pi.exe" } else { "pi" };
    let search_path = env::var_os("PATH");
    let home = dirs::home_dir();
    pi_binary_candidates(
        executable_name,
        search_path.as_deref(),
        home.as_deref(),
        cfg!(unix),
        cfg!(target_os = "macos"),
    )
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn pi_binary_candidates(
    executable_name: &str,
    search_path: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
    unix_like: bool,
    macos: bool,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = search_path {
        candidates.extend(env::split_paths(&path).map(|directory| directory.join(executable_name)));
    }
    if unix_like {
        candidates.extend([
            PathBuf::from("/usr/local/bin").join(executable_name),
            PathBuf::from("/usr/bin").join(executable_name),
        ]);
    }
    if macos {
        candidates.push(PathBuf::from("/opt/homebrew/bin/pi"));
    }
    if let Some(home) = home {
        candidates.extend([
            home.join(".local/bin").join(executable_name),
            home.join(".local/share/pnpm").join(executable_name),
            home.join(".npm-global/bin").join(executable_name),
            home.join(".volta/bin").join(executable_name),
            home.join(".asdf/shims").join(executable_name),
            home.join(".bun/bin").join(executable_name),
            home.join("Library/pnpm").join(executable_name),
        ]);
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut nvm_candidates: Vec<_> = entries
                .flatten()
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
    candidates
}

fn pi_is_running() -> bool {
    let system = System::new_all();
    system.processes().values().any(is_pi_process)
}

fn is_pi_process(process: &sysinfo::Process) -> bool {
    is_pi_process_shape(
        &process.name().to_string_lossy(),
        process.exe(),
        process.cmd(),
    )
}

fn is_pi_process_shape(
    name: &str,
    executable: Option<&Path>,
    command: &[std::ffi::OsString],
) -> bool {
    let fixed_names = [
        Some(name.to_ascii_lowercase()),
        executable.map(|value| value.to_string_lossy().to_ascii_lowercase()),
        command
            .first()
            .and_then(|value| Path::new(value).file_name())
            .map(|value| value.to_string_lossy().to_ascii_lowercase()),
    ];
    if fixed_names
        .into_iter()
        .flatten()
        .any(|value| value == "pi" || value == "pi.exe")
    {
        return true;
    }
    // The official entry point sets its own process title, but SDK or wrapper launches keep the
    // interpreter name; recognize them through the packaged script path instead.
    let interpreter = command
        .first()
        .and_then(|value| Path::new(value).file_name())
        .map(|value| value.to_string_lossy().to_ascii_lowercase());
    matches!(interpreter.as_deref(), Some("node" | "node.exe"))
        && command
            .iter()
            .skip(1)
            .any(|argument| argument.to_string_lossy().contains("pi-coding-agent"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
    const CHILD_ID: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn binary_candidates_cover_linux_desktop_install_locations() {
        let home = tempfile::tempdir().expect("binary fixture home");
        let candidates = pi_binary_candidates("pi", None, Some(home.path()), true, false);

        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/pi")));
        assert!(candidates.contains(&PathBuf::from("/usr/bin/pi")));
        assert!(candidates.contains(&home.path().join(".local/bin/pi")));
        assert!(candidates.contains(&home.path().join(".local/share/pnpm/pi")));
        assert!(candidates.contains(&home.path().join(".npm-global/bin/pi")));
    }

    #[tokio::test]
    async fn scans_recognized_sessions_cache_and_protected_data_without_retaining_bodies() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        write_session(
            &bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl")),
            None,
        );
        fs::write(home.join("auth.json"), b"top-secret-oauth-token").expect("auth fixture");
        fs::write(home.join("models-store.json"), b"{\"providers\":{}}").expect("catalog cache");
        fs::write(home.join("settings.json"), b"{\"theme\":\"light\"}").expect("settings");

        let snapshot = PiAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan pi fixture");

        assert_eq!(snapshot.installation.kind, AgentKind::Pi);
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].name, "Named pi session");
        assert_eq!(snapshot.sessions[0].cwd, fixture_project_cwd());
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].roots, vec![fixture_project_cwd()]);
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.id == "cache:pi-models-store"
                    && item.category == StorageCategory::Cache)
        );
        assert!(snapshot.items.iter().any(|item| item.protected));
        let serialized = serde_json::to_string(&snapshot).expect("snapshot JSON");
        assert!(!serialized.contains("PRIVATE_TRANSCRIPT_BODY"));
        assert!(!serialized.contains("top-secret-oauth-token"));
        assert!(
            snapshot
                .items
                .iter()
                .find(|item| item.thread_id.is_some())
                .expect("session item")
                .metadata
                .contains_key("sourceRevision")
        );
    }

    #[tokio::test]
    async fn unknown_session_headers_do_not_block_the_filesystem_inventory() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        fs::write(bucket.join("mystery.jsonl"), b"not a pi session\n").expect("unknown file");
        fs::write(home.join("models-store.json"), b"{}").expect("catalog cache");

        let snapshot = PiAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan pi fixture");

        assert!(snapshot.sessions.is_empty());
        assert!(!snapshot.warnings.is_empty());
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.category == StorageCategory::Cache)
        );
    }

    #[test]
    fn unnamed_sessions_use_the_bounded_first_user_message_as_their_title() {
        let metadata = SessionFileMetadata {
            id: SESSION_ID.into(),
            cwd: "/tmp/project".into(),
            created_at: parse_rfc3339_timestamp("2026-08-27T10:00:00.000Z"),
            name: None,
            first_user_message: Some("Fix the session title".into()),
            parent_session: None,
        };

        assert_eq!(session_display_name(&metadata), "Fix the session title");

        let long_title = format!("{} private suffix", "x".repeat(SESSION_TITLE_CHAR_LIMIT));
        let bounded = normalized_session_title(&long_title).expect("bounded title");
        assert_eq!(bounded.chars().count(), SESSION_TITLE_CHAR_LIMIT + 1);
        assert!(bounded.ends_with('…'));
        assert!(!bounded.contains("private suffix"));
    }

    #[test]
    fn metadata_reader_matches_pi_first_message_fallback_and_latest_name_clear() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let path = fixture.path().join("session.jsonl");
        let contents = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{SESSION_ID}\",\"timestamp\":\"2026-08-27T10:00:00.000Z\",\"cwd\":\"/tmp/project\"}}\n\
             {{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"timestamp\":\"2026-08-27T10:00:01.000Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"  Fix Pi titles\\nwithout retaining the whole transcript  \"}}],\"timestamp\":1787839200000}}}}\n\
             {{\"type\":\"session_info\",\"id\":\"b2\",\"parentId\":\"a1\",\"timestamp\":\"2026-08-27T10:00:02.000Z\",\"name\":\"Old name\"}}\n\
             {{\"type\":\"session_info\",\"id\":\"c3\",\"parentId\":\"b2\",\"timestamp\":\"2026-08-27T10:00:03.000Z\",\"name\":\" \"}}\n"
        );
        fs::write(&path, contents).expect("session fixture");

        let metadata = read_session_metadata(&path)
            .expect("metadata")
            .expect("recognized session");

        assert_eq!(metadata.name, None);
        assert_eq!(
            metadata.first_user_message.as_deref(),
            Some("Fix Pi titles without retaining the whole transcript")
        );
        assert_eq!(
            session_display_name(&metadata),
            "Fix Pi titles without retaining the whole transcript"
        );
    }

    #[tokio::test]
    async fn links_forked_sessions_to_their_parent_file_without_deletion_cascade() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        let parent_path = bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl"));
        let child_path = bucket.join(format!("2026-08-27T11-00-00-000Z_{CHILD_ID}.jsonl"));
        write_session(&parent_path, None);
        write_session(&child_path, Some(&parent_path));

        let snapshot = PiAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan pi fixture");

        let child = snapshot
            .sessions
            .iter()
            .find(|session| session.id == CHILD_ID)
            .expect("forked session");
        assert_eq!(child.parent_thread_id.as_deref(), Some(SESSION_ID));
        assert!(child.descendant_ids.is_empty());
        let parent = snapshot
            .sessions
            .iter()
            .find(|session| session.id == SESSION_ID)
            .expect("parent session");
        assert!(parent.descendant_ids.is_empty());
    }

    #[tokio::test]
    async fn marks_every_mutable_item_blocked_while_pi_is_running() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        write_session(
            &bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl")),
            None,
        );
        fs::write(home.join("models-store.json"), b"{}").expect("catalog cache");
        let mut warnings = Vec::new();
        let mut sessions = Vec::new();
        let mut items = Vec::new();
        let mut projects = Vec::new();

        scan_sessions(
            &home,
            true,
            &mut sessions,
            &mut items,
            &mut projects,
            &mut warnings,
        )
        .expect("scan with a running writer");

        assert!(
            items
                .iter()
                .filter(|item| !item.protected)
                .all(|item| item.blocked_reason.is_some())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinked_session_files_without_following_them() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let outside = tempfile::tempdir().expect("outside fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        let outside_session = outside.path().join("outside.jsonl");
        write_session(&outside_session, None);
        std::os::unix::fs::symlink(
            &outside_session,
            bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl")),
        )
        .expect("symlink");

        let snapshot = PiAdapter::new()
            .scan(home.to_str())
            .await
            .expect("safe scan");

        assert!(snapshot.sessions.is_empty());
        assert!(!snapshot.warnings.is_empty());
        assert!(outside_session.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_paths_stay_beneath_the_configured_agent_directory() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let real = fixture.path().join("real-agent");
        let bucket = real.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        write_session(
            &bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl")),
            None,
        );
        let linked_home = fixture.path().join("linked-agent");
        std::os::unix::fs::symlink(&real, &linked_home).expect("home symlink");

        let snapshot = PiAdapter::new()
            .scan(linked_home.to_str())
            .await
            .expect("scan through the configured path");

        let item = snapshot
            .items
            .iter()
            .find(|item| item.thread_id.is_some())
            .expect("session item");
        assert!(Path::new(&item.paths[0]).starts_with(&linked_home));
    }

    #[tokio::test]
    async fn loads_content_only_after_an_explicit_item_request() {
        let fixture = tempfile::tempdir().expect("pi fixture");
        let home = fixture.path().join("agent");
        let bucket = home.join("sessions/--tmp-project--");
        fs::create_dir_all(&bucket).expect("session bucket");
        write_session(
            &bucket.join(format!("2026-08-27T10-00-00-000Z_{SESSION_ID}.jsonl")),
            None,
        );
        let adapter = PiAdapter::new();
        let snapshot = adapter.scan(home.to_str()).await.expect("scan");
        let item = snapshot
            .items
            .iter()
            .find(|item| item.thread_id.as_deref() == Some(SESSION_ID))
            .expect("session item");

        let detail = adapter
            .load_item_content(&snapshot.installation, item)
            .await
            .expect("detail");

        assert_eq!(detail.source, "piSession.readOnly");
        assert!(detail.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Message { text, .. } if text.contains("PRIVATE_TRANSCRIPT_BODY")
        )));
        assert!(detail.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Text { title, .. } if title == "bash"
        )));
    }

    #[test]
    fn pi_process_detection_matches_the_official_entry_points() {
        let node_entry: Vec<std::ffi::OsString> = vec![
            std::ffi::OsString::from("/usr/local/bin/node"),
            std::ffi::OsString::from(
                "/opt/pi/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
            ),
        ];
        assert!(is_pi_process_shape(
            "node",
            Some(Path::new("/usr/local/bin/node")),
            &node_entry,
        ));
        assert!(is_pi_process_shape(
            "pi",
            Some(Path::new("/Users/demo/.nvm/versions/node/v24.14.1/bin/pi")),
            &[std::ffi::OsString::from("pi")],
        ));
        assert!(!is_pi_process_shape(
            "cargo",
            Some(Path::new("/Users/demo/.cargo/bin/cargo")),
            &[
                std::ffi::OsString::from("cargo"),
                std::ffi::OsString::from("test"),
                std::ffi::OsString::from("adapter-pi"),
            ],
        ));
        assert!(!is_pi_process_shape(
            "bash",
            Some(Path::new("/bin/bash")),
            &[
                std::ffi::OsString::from("/bin/bash"),
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from("echo pi-coding-agent"),
            ],
        ));
    }

    #[ignore = "requires a local pi installation"]
    #[tokio::test]
    async fn live_scans_local_pi_metadata_without_mutation() {
        let adapter = PiAdapter::new();
        let snapshot = adapter.scan(None).await.expect("scan local pi");
        assert_eq!(snapshot.installation.kind, AgentKind::Pi);
        assert!(
            snapshot
                .items
                .iter()
                .flat_map(|item| &item.paths)
                .all(|path| Path::new(path).starts_with(&snapshot.installation.home))
        );
    }

    fn write_session(path: &Path, parent_session: Option<&Path>) {
        let parent = parent_session
            .map(|path| {
                format!(
                    ",\"parentSession\":{}",
                    serde_json::to_string(&path.to_string_lossy()).expect("parent json")
                )
            })
            .unwrap_or_default();
        let cwd = serde_json::to_string(fixture_project_cwd()).expect("cwd json");
        let contents = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{SESSION_ID_PLACEHOLDER}\",\"timestamp\":\"2026-08-27T10:00:00.000Z\",\"cwd\":{cwd}{parent}}}\n\
             {{\"type\":\"message\",\"id\":\"a1b2c3d4\",\"parentId\":null,\"timestamp\":\"2026-08-27T10:00:01.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"PRIVATE_TRANSCRIPT_BODY\",\"timestamp\":1787839200000}}}}\n\
             {{\"type\":\"session_info\",\"id\":\"e5f6g7h8\",\"parentId\":\"a1b2c3d4\",\"timestamp\":\"2026-08-27T10:00:02.000Z\",\"name\":\"Named pi session\"}}\n\
             {{\"type\":\"message\",\"id\":\"b2c3d4e5\",\"parentId\":\"e5f6g7h8\",\"timestamp\":\"2026-08-27T10:00:03.000Z\",\"message\":{{\"role\":\"bashExecution\",\"command\":\"ls ~/.pi/agent\",\"output\":\"sessions settings.json\",\"exitCode\":0,\"cancelled\":false,\"truncated\":false,\"timestamp\":1787839203000}}}}\n",
            SESSION_ID_PLACEHOLDER = if parent_session.is_some() {
                CHILD_ID
            } else {
                SESSION_ID
            },
        );
        fs::write(path, contents).expect("session fixture");
    }

    fn fixture_project_cwd() -> &'static str {
        if cfg!(windows) {
            r"C:\tmp\project"
        } else {
            "/tmp/project"
        }
    }
}
