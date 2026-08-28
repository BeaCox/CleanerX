//! Claude Code storage discovery and bounded, local-only content access.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead as _, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentCapabilities, AgentDetectionState, AgentInstallation, AgentKind,
    CategorySummary, CleanerError, CleanupItem, ContentBlock, InventorySnapshot, ItemContentDetail,
    ItemThumbnail, MemoryCapabilities, MemoryScope, ProjectGroup, RiskLevel, SessionRecord,
    StorageCategory,
};
use serde::Deserialize;
use serde_json::Value;
use sysinfo::System;
use tokio::process::Command;
use uuid::Uuid;

const METADATA_SCAN_LIMIT: u64 = 2 * 1024 * 1024;
const CONTENT_TEXT_LIMIT: usize = 512 * 1024;
const CONTENT_BLOCK_LIMIT: usize = 200;

const PROTECTED_NAMES: &[&str] = &[
    ".credentials.json",
    "CLAUDE.md",
    "settings.json",
    "settings.local.json",
    "keybindings.json",
    "plugins",
    "skills",
    "commands",
    "agents",
    "rules",
    "hooks",
    "themes",
    "backups",
    "remote-settings.json",
    "policy-limits.json",
];

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    fn resolve_home(&self, custom_home: Option<&str>) -> Result<PathBuf, CleanerError> {
        let path = custom_home
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
            .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
            .ok_or_else(|| CleanerError::NotFound("Claude Code configuration directory".into()))?;
        if !path.is_absolute() {
            return Err(CleanerError::InvalidRequest(
                "Claude Code home override must be an absolute path".into(),
            ));
        }
        Ok(path)
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    async fn detect(&self, custom_home: Option<&str>) -> Result<AgentInstallation, CleanerError> {
        let home = self.resolve_home(custom_home)?;
        let binary = find_claude_binary();
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
        let running = claude_is_running(&home);
        let mut warnings = Vec::new();
        if !home.exists() {
            warnings.push(format!(
                "Claude Code home does not exist: {}",
                home.display()
            ));
        }
        if binary.is_none() {
            warnings.push(
                "Claude Code executable was not found; only recognized local storage can be reported"
                    .into(),
            );
        }

        let recognized_storage = home.is_dir();
        Ok(AgentInstallation {
            kind: AgentKind::ClaudeCode,
            state: AgentDetectionState::from_presence(binary.is_some(), recognized_storage),
            home: home.to_string_lossy().into_owned(),
            binary: binary.map(|path| path.to_string_lossy().into_owned()),
            version,
            app_support: None,
            running,
            capabilities: AgentCapabilities {
                thread_list: recognized_storage,
                thread_delete: recognized_storage,
                memory: MemoryCapabilities {
                    can_scan: recognized_storage,
                    can_read_content: recognized_storage,
                    can_reset_scope: recognized_storage,
                    scope: MemoryScope::Project,
                    ..MemoryCapabilities::default()
                },
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
        let active_ids = active_session_ids(&home, &mut warnings);
        let mut sessions = Vec::new();
        let mut items = Vec::new();
        let mut session_buckets = HashMap::new();

        scan_project_sessions(
            &home,
            installation.running,
            &active_ids,
            &mut sessions,
            &mut items,
            &mut session_buckets,
            &mut warnings,
        )?;
        let projects = group_projects(&sessions, &session_buckets);
        let project_by_bucket: HashMap<PathBuf, String> = projects
            .iter()
            .flat_map(|project| {
                project.session_ids.iter().filter_map(|session_id| {
                    session_buckets
                        .get(session_id)
                        .map(|bucket| (bucket.clone(), project.id.clone()))
                })
            })
            .collect();
        scan_project_memory(
            &home,
            installation.running,
            &projects,
            &project_by_bucket,
            &mut items,
            &mut warnings,
        )?;
        scan_application_data(&home, installation.running, &mut items, &mut warnings)?;
        scan_protected(&home, &mut items, &mut warnings)?;

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
                "CleanerX never opens Claude Code authentication, settings, instructions, skills, or plugin data.",
            ));
        }
        match item.category {
            StorageCategory::Session | StorageCategory::ArchivedSession => {
                content_from_transcript(item)
            }
            StorageCategory::Memory => content_from_memory(item),
            _ => Ok(content_notice(
                item,
                "filesystem.metadataOnly",
                "This recognized Claude Code application-data item is inventoried by metadata only.",
            )),
        }
    }

    async fn load_item_thumbnail(
        &self,
        _installation: &AgentInstallation,
        _item: &CleanupItem,
    ) -> Result<Option<ItemThumbnail>, CleanerError> {
        Err(CleanerError::Unsupported(
            "Claude Code inventory does not expose thumbnail items".into(),
        ))
    }

    async fn delete_sessions(
        &self,
        _installation: &AgentInstallation,
        _session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError> {
        Err(CleanerError::Unsupported(
            "Claude Code session removal requires CleanerX's preflighted path transaction".into(),
        ))
    }

    async fn reset_memory(&self, _installation: &AgentInstallation) -> Result<(), CleanerError> {
        Err(CleanerError::Unsupported(
            "Claude Code project memory removal requires CleanerX's preflighted path transaction"
                .into(),
        ))
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TranscriptEnvelope {
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "session_id")]
    legacy_session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default)]
    ai_title: Option<String>,
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Debug, Default)]
struct TranscriptMetadata {
    cwd: String,
    source: String,
    title: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

fn scan_project_sessions(
    home: &Path,
    running: bool,
    active_ids: &HashSet<String>,
    sessions: &mut Vec<SessionRecord>,
    items: &mut Vec<CleanupItem>,
    session_buckets: &mut HashMap<String, PathBuf>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let projects_root = home.join("projects");
    let Ok(buckets) = fs::read_dir(&projects_root) else {
        return Ok(());
    };
    for bucket in buckets.flatten() {
        let bucket_path = bucket.path();
        if !is_plain_directory(&bucket_path)? {
            warnings.push(format!(
                "Skipped unrecognized or linked Claude project bucket: {}",
                bucket_path.display()
            ));
            continue;
        }
        let Ok(entries) = fs::read_dir(&bucket_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let transcript = entry.path();
            if transcript.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(session_id) = transcript.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if Uuid::parse_str(session_id).is_err() || !is_plain_file(&transcript)? {
                warnings.push(format!(
                    "Skipped unrecognized or linked Claude transcript: {}",
                    transcript.display()
                ));
                continue;
            }
            let Some(metadata) = read_transcript_metadata(&transcript, session_id)? else {
                warnings.push(format!(
                    "Skipped Claude transcript with unrecognized session metadata: {}",
                    transcript.display()
                ));
                continue;
            };
            let associated = associated_session_paths(home, &bucket_path, session_id);
            let path_buffers: Vec<PathBuf> = std::iter::once(transcript.clone())
                .chain(associated)
                .collect();
            let (source_revision, unsafe_reason) = source_revision(&path_buffers, warnings);
            let paths: Vec<String> = path_buffers
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let size_bytes = paths
                .iter()
                .filter_map(|path| cleanerx_core::safety::allocated_size(Path::new(path)).ok())
                .sum();
            let active = active_ids.contains(session_id);
            let blocked_reason = running
                .then(|| {
                    "Claude Code is running; quit it before deleting local session data".into()
                })
                .or(unsafe_reason);
            let name = metadata
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Claude session {}", &session_id[..8]));
            let record = SessionRecord {
                id: session_id.to_owned(),
                name: name.clone(),
                cwd: metadata.cwd.clone(),
                path: Some(transcript.to_string_lossy().into_owned()),
                source: metadata.source.clone(),
                archived: false,
                pinned: false,
                status: if active { "active" } else { "notLoaded" }.into(),
                created_at: metadata.created_at,
                updated_at: metadata.updated_at.or_else(|| modified_at(&transcript)),
                size_bytes,
                parent_thread_id: None,
                descendant_ids: Vec::new(),
            };
            session_buckets.insert(session_id.to_owned(), bucket_path.clone());
            items.push(CleanupItem {
                id: format!("session:{session_id}"),
                category: StorageCategory::Session,
                title: name,
                subtitle: (!metadata.cwd.is_empty()).then_some(metadata.cwd),
                paths,
                project_id: Some(project_id_for_bucket(&bucket_path)),
                thread_id: Some(session_id.to_owned()),
                size_bytes,
                modified_at: record.updated_at,
                risk: RiskLevel::High,
                recoverable: true,
                default_selected: false,
                protected: false,
                blocked_reason,
                metadata: BTreeMap::from([
                    ("source".into(), record.source.clone()),
                    ("status".into(), record.status.clone()),
                    ("pinned".into(), "false".into()),
                    ("requiresAgentExit".into(), "true".into()),
                ])
                .into_iter()
                .chain(source_revision.map(|revision| ("sourceRevision".into(), revision)))
                .collect(),
            });
            sessions.push(record);
        }
    }
    Ok(())
}

fn read_transcript_metadata(
    path: &Path,
    expected_session_id: &str,
) -> Result<Option<TranscriptMetadata>, CleanerError> {
    let file = fs::File::open(path)?;
    let reader = std::io::BufReader::new(file).take(METADATA_SCAN_LIMIT);
    let mut recognized = false;
    let mut metadata = TranscriptMetadata::default();
    let stream = serde_json::Deserializer::from_reader(reader).into_iter::<TranscriptEnvelope>();
    for envelope in stream.flatten() {
        let session_id = match (
            envelope.session_id.as_deref(),
            envelope.legacy_session_id.as_deref(),
        ) {
            (Some(primary), Some(legacy)) if primary != legacy => continue,
            (Some(primary), _) => Some(primary),
            (_, Some(legacy)) => Some(legacy),
            _ => None,
        };
        if session_id == Some(expected_session_id) {
            recognized = true;
            if metadata.cwd.is_empty()
                && let Some(cwd) = envelope.cwd.filter(|value| Path::new(value).is_absolute())
            {
                metadata.cwd = cwd;
            }
            if metadata.source.is_empty()
                && let Some(source) = envelope.entrypoint.filter(|value| !value.trim().is_empty())
            {
                metadata.source = source;
            }
            metadata.title = envelope
                .custom_title
                .or(envelope.ai_title)
                .or(envelope.summary)
                .filter(|value| !value.trim().is_empty())
                .or(metadata.title);
            if let Some(timestamp) = envelope
                .timestamp
                .as_deref()
                .and_then(parse_rfc3339_timestamp)
            {
                metadata.created_at = Some(
                    metadata
                        .created_at
                        .map_or(timestamp, |old| old.min(timestamp)),
                );
                metadata.updated_at = Some(
                    metadata
                        .updated_at
                        .map_or(timestamp, |old| old.max(timestamp)),
                );
            }
        }
    }
    if metadata.source.is_empty() {
        metadata.source = "cli".into();
    }
    Ok(recognized.then_some(metadata))
}

fn group_projects(
    sessions: &[SessionRecord],
    session_buckets: &HashMap<String, PathBuf>,
) -> Vec<ProjectGroup> {
    let mut groups: BTreeMap<String, ProjectGroup> = BTreeMap::new();
    for session in sessions {
        let Some(bucket) = session_buckets.get(&session.id) else {
            continue;
        };
        let id = project_id_for_bucket(bucket);
        let root = session.cwd.trim();
        let group = groups.entry(id.clone()).or_insert_with(|| ProjectGroup {
            id,
            name: project_name(root, bucket),
            roots: Vec::new(),
            session_ids: Vec::new(),
            size_bytes: 0,
        });
        if Path::new(root).is_absolute() && !group.roots.iter().any(|known| known == root) {
            group.roots.push(root.to_owned());
        }
        group.session_ids.push(session.id.clone());
        group.size_bytes = group.size_bytes.saturating_add(session.size_bytes);
    }
    groups.into_values().collect()
}

fn scan_project_memory(
    home: &Path,
    running: bool,
    projects: &[ProjectGroup],
    project_by_bucket: &HashMap<PathBuf, String>,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let root = home.join("projects");
    let Ok(buckets) = fs::read_dir(root) else {
        return Ok(());
    };
    for bucket in buckets.flatten() {
        let bucket_path = bucket.path();
        if !is_plain_directory(&bucket_path)? {
            continue;
        }
        let memory = bucket_path.join("memory");
        if !memory.exists() {
            continue;
        }
        if !is_plain_directory(&memory)? {
            warnings.push(format!(
                "Skipped linked or unrecognized Claude memory directory: {}",
                memory.display()
            ));
            continue;
        }
        let project_id = project_by_bucket.get(&bucket_path).cloned();
        let project_name = project_id
            .as_deref()
            .and_then(|id| projects.iter().find(|project| project.id == id))
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Unlinked Claude project".into());
        let size_bytes = cleanerx_core::safety::allocated_size(&memory)?;
        let (source_revision, unsafe_reason) =
            source_revision(std::slice::from_ref(&memory), warnings);
        items.push(CleanupItem {
            id: format!("memory:{}", project_id_for_bucket(&bucket_path)),
            category: StorageCategory::Memory,
            title: format!("{project_name} memory"),
            subtitle: Some("Claude Code project auto memory".into()),
            paths: vec![memory.to_string_lossy().into_owned()],
            project_id,
            thread_id: None,
            size_bytes,
            modified_at: newest_modified_markdown(&memory),
            risk: RiskLevel::High,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason: running
                .then(|| "Claude Code is running; quit it before deleting project memory".into())
                .or(unsafe_reason),
            metadata: BTreeMap::from([
                ("scope".into(), "project".into()),
                ("requiresAgentExit".into(), "true".into()),
            ])
            .into_iter()
            .chain(source_revision.map(|revision| ("sourceRevision".into(), revision)))
            .collect(),
        });
    }
    Ok(())
}

fn scan_application_data(
    home: &Path,
    running: bool,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let definitions = [
        (
            "claude-history",
            "Prompt history",
            StorageCategory::Log,
            RiskLevel::Review,
            true,
            vec![home.join("history.jsonl")],
        ),
        (
            "claude-cache",
            "Claude Code caches",
            StorageCategory::Cache,
            RiskLevel::Safe,
            false,
            vec![
                home.join("cache"),
                home.join("paste-cache"),
                home.join("stats-cache.json"),
                home.join("usage-data"),
            ],
        ),
        (
            "claude-temporary",
            "Claude Code temporary data",
            StorageCategory::Temporary,
            RiskLevel::Safe,
            false,
            vec![
                home.join("plans"),
                home.join("shell-snapshots"),
                home.join("todos"),
                home.join("logs"),
            ],
        ),
    ];
    for (id, title, category, risk, recoverable, candidates) in definitions {
        let mut paths = Vec::new();
        let mut size_bytes = 0_u64;
        for path in candidates {
            if !path.exists() {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                warnings.push(format!(
                    "Skipped linked Claude application data: {}",
                    path.display()
                ));
                continue;
            }
            size_bytes = size_bytes.saturating_add(cleanerx_core::safety::allocated_size(&path)?);
            paths.push(path.to_string_lossy().into_owned());
        }
        if paths.is_empty() {
            continue;
        }
        let path_buffers: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let (source_revision, unsafe_reason) = source_revision(&path_buffers, warnings);
        items.push(CleanupItem {
            id: id.into(),
            category,
            title: title.into(),
            subtitle: Some("Recognized Claude Code application data".into()),
            modified_at: newest_modified_strings(&paths),
            paths,
            project_id: None,
            thread_id: None,
            size_bytes,
            risk,
            recoverable,
            default_selected: false,
            protected: false,
            blocked_reason: running
                .then(|| {
                    "Claude Code is running; quit it before cleaning writable application data"
                        .into()
                })
                .or(unsafe_reason),
            metadata: BTreeMap::from([
                ("requiresAgentExit".into(), "true".into()),
                ("regenerable".into(), (!recoverable).to_string()),
            ])
            .into_iter()
            .chain(source_revision.map(|revision| ("sourceRevision".into(), revision)))
            .collect(),
        });
    }
    Ok(())
}

fn scan_protected(
    home: &Path,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    for name in PROTECTED_NAMES {
        let path = home.join(name);
        if !path.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            warnings.push(format!(
                "Protected Claude path is a symbolic link and was not followed: {}",
                path.display()
            ));
        }
        items.push(CleanupItem {
            id: format!("protected:claude:{name}"),
            category: StorageCategory::Protected,
            title: (*name).into(),
            subtitle: Some("Claude Code configuration or credentials".into()),
            paths: vec![path.to_string_lossy().into_owned()],
            project_id: None,
            thread_id: None,
            size_bytes: if metadata.file_type().is_symlink() {
                0
            } else {
                cleanerx_core::safety::allocated_size(&path)?
            },
            modified_at: modified_at(&path),
            risk: RiskLevel::Protected,
            recoverable: false,
            default_selected: false,
            protected: true,
            blocked_reason: Some("Protected Claude Code data".into()),
            metadata: BTreeMap::from([("protection".into(), "always".into())]),
        });
    }
    Ok(())
}

fn active_session_ids(home: &Path, warnings: &mut Vec<String>) -> HashSet<String> {
    let mut ids = HashSet::new();
    let root = home.join("sessions");
    let Ok(entries) = fs::read_dir(root) else {
        return ids;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            warnings.push(format!(
                "Ignored linked Claude running-session marker: {}",
                path.display()
            ));
            continue;
        }
        if let Some(id) = path.file_stem().and_then(|value| value.to_str())
            && Uuid::parse_str(id).is_ok()
        {
            ids.insert(id.to_owned());
        }
    }
    ids
}

fn associated_session_paths(home: &Path, bucket: &Path, session_id: &str) -> Vec<PathBuf> {
    [
        bucket.join(session_id),
        home.join("tasks").join(session_id),
        home.join("file-history").join(session_id),
        home.join("image-cache").join(session_id),
        home.join("uploads").join(session_id),
        home.join("session-env").join(session_id),
        home.join("debug").join(session_id),
        home.join("debug").join(format!("{session_id}.txt")),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn source_revision(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
) -> (Option<String>, Option<String>) {
    match cleanerx_core::metadata_revision(paths) {
        Ok(revision) => (Some(revision), None),
        Err(error) => {
            warnings.push(format!(
                "Claude data was made read-only by a safety check: {error}"
            ));
            (
                None,
                Some(
                    "Linked, foreign-owned, or unstable Claude Code data cannot be cleaned".into(),
                ),
            )
        }
    }
}

fn validate_content_paths(
    installation: &AgentInstallation,
    item: &CleanupItem,
) -> Result<(), CleanerError> {
    if installation.kind != AgentKind::ClaudeCode {
        return Err(CleanerError::InvalidRequest(
            "Claude content request used a different Agent installation".into(),
        ));
    }
    let roots = vec![PathBuf::from(&installation.home)];
    for raw_path in &item.paths {
        let path = Path::new(raw_path);
        if !path.exists() {
            continue;
        }
        cleanerx_core::validate_existing_beneath(path, &roots)?;
    }
    Ok(())
}

fn content_from_transcript(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let path = item
        .paths
        .iter()
        .map(Path::new)
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .ok_or_else(|| CleanerError::NotFound("Claude transcript file".into()))?;
    let file = fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file).take(CONTENT_TEXT_LIMIT as u64);
    let mut line = String::new();
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "claudeTranscript.readOnly".into(),
        truncated: fs::metadata(path)?.len() > CONTENT_TEXT_LIMIT as u64,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    while detail.blocks.len() < CONTENT_BLOCK_LIMIT && reader.read_line(&mut line)? > 0 {
        detail.bytes_read = detail.bytes_read.saturating_add(line.len() as u64);
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && let Some(message) = value.get("message")
            && let Some(role) = message.get("role").and_then(Value::as_str)
        {
            let text = message_text(message.get("content"));
            if !text.trim().is_empty() {
                detail.blocks.push(ContentBlock::Message {
                    role: role.to_owned(),
                    text: bounded_string(text, CONTENT_TEXT_LIMIT / 2),
                    phase: None,
                });
            }
        }
        line.clear();
    }
    if detail.blocks.len() >= CONTENT_BLOCK_LIMIT {
        detail.truncated = true;
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "No supported message blocks were found in this transcript preview.".into(),
        });
    }
    Ok(detail)
}

fn content_from_memory(item: &CleanupItem) -> Result<ItemContentDetail, CleanerError> {
    let root = item
        .paths
        .first()
        .map(Path::new)
        .ok_or_else(|| CleanerError::NotFound("Claude memory directory".into()))?;
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if !is_plain_file(&path)? {
            return Err(CleanerError::UnsafePath(
                path.to_string_lossy().into_owned(),
            ));
        }
        files.push(path);
    }
    files.sort();
    let mut remaining = CONTENT_TEXT_LIMIT;
    let mut detail = ItemContentDetail {
        item_id: item.id.clone(),
        source: "claudeMemoryMarkdown.readOnly".into(),
        truncated: false,
        bytes_read: 0,
        blocks: Vec::new(),
        warning: None,
    };
    for path in files.into_iter().take(CONTENT_BLOCK_LIMIT) {
        if remaining == 0 {
            detail.truncated = true;
            break;
        }
        let metadata = fs::metadata(&path)?;
        let mut text = String::new();
        fs::File::open(&path)?
            .take(remaining as u64)
            .read_to_string(&mut text)?;
        let read = text.len();
        remaining = remaining.saturating_sub(read);
        detail.bytes_read = detail.bytes_read.saturating_add(read as u64);
        detail.truncated |= metadata.len() > read as u64;
        detail.blocks.push(ContentBlock::Text {
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "memory".into()),
            text,
        });
    }
    if detail.blocks.is_empty() {
        detail.blocks.push(ContentBlock::Notice {
            text: "This Claude Code project memory directory contains no Markdown entries.".into(),
        });
    }
    Ok(detail)
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

fn message_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_str().map(str::to_owned).or_else(|| {
                    part.as_object()
                        .filter(|object| object.get("type").and_then(Value::as_str) == Some("text"))
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
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

fn project_id_for_bucket(bucket: &Path) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, bucket.to_string_lossy().as_bytes()).to_string()
}

fn project_name(root: &str, bucket: &Path) -> String {
    Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            bucket
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Claude project".into())
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

fn newest_modified_strings(paths: &[String]) -> Option<DateTime<Utc>> {
    paths
        .iter()
        .filter_map(|path| modified_at(Path::new(path)))
        .max()
}

fn newest_modified_markdown(root: &Path) -> Option<DateTime<Utc>> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .filter_map(|path| modified_at(&path))
        .max()
        .or_else(|| modified_at(root))
}

fn find_claude_binary() -> Option<PathBuf> {
    let executable_names = if cfg!(windows) {
        &["claude.exe", "claude.cmd", "claude.bat"][..]
    } else {
        &["claude"][..]
    };
    let search_path = env::var_os("PATH");
    let home = dirs::home_dir();
    claude_binary_candidates(
        executable_names,
        BinarySearchContext {
            search_path: search_path.as_deref(),
            home: home.as_deref(),
            data_dir: dirs::data_dir().as_deref(),
            local_data_dir: dirs::data_local_dir().as_deref(),
            unix_like: cfg!(unix),
            macos: cfg!(target_os = "macos"),
            windows: cfg!(windows),
        },
    )
    .into_iter()
    .find(|candidate| candidate.is_file())
}

#[derive(Clone, Copy)]
struct BinarySearchContext<'a> {
    search_path: Option<&'a std::ffi::OsStr>,
    home: Option<&'a Path>,
    data_dir: Option<&'a Path>,
    local_data_dir: Option<&'a Path>,
    unix_like: bool,
    macos: bool,
    windows: bool,
}

fn claude_binary_candidates(
    executable_names: &[&str],
    context: BinarySearchContext<'_>,
) -> Vec<PathBuf> {
    let BinarySearchContext {
        search_path,
        home,
        data_dir,
        local_data_dir,
        unix_like,
        macos,
        windows,
    } = context;
    let mut candidates = Vec::new();
    if let Some(path) = search_path {
        for directory in env::split_paths(&path) {
            candidates.extend(executable_names.iter().map(|name| directory.join(name)));
        }
    }
    if unix_like {
        for directory in [Path::new("/usr/local/bin"), Path::new("/usr/bin")] {
            candidates.extend(executable_names.iter().map(|name| directory.join(name)));
        }
    }
    if macos {
        candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    }
    if let Some(home) = home {
        for directory in [
            home.join(".local/bin"),
            home.join(".local/share/pnpm"),
            home.join(".npm-global/bin"),
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
            home.join(".bun/bin"),
            home.join("Library/pnpm"),
        ] {
            candidates.extend(executable_names.iter().map(|name| directory.join(name)));
        }
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut nvm_candidates: Vec<_> = entries
                .flatten()
                .flat_map(|entry| {
                    let directory = entry.path().join("bin");
                    executable_names
                        .iter()
                        .map(move |name| directory.join(name))
                })
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
    if windows {
        for directory in [
            data_dir.map(|root| root.join("npm")),
            data_dir.map(|root| root.join("pnpm")),
            local_data_dir.map(|root| root.join("pnpm")),
        ]
        .into_iter()
        .flatten()
        {
            candidates.extend(executable_names.iter().map(|name| directory.join(name)));
        }
    }
    candidates
}

fn claude_is_running(home: &Path) -> bool {
    if fs::read_dir(home.join("sessions"))
        .ok()
        .is_some_and(|mut entries| entries.next().is_some())
    {
        return true;
    }
    let system = System::new_all();
    system.processes().values().any(|process| {
        is_claude_process_shape(
            &process.name().to_string_lossy(),
            process.exe(),
            process.cmd(),
        )
    })
}

fn is_claude_process_shape(
    name: &str,
    executable: Option<&Path>,
    command: &[std::ffi::OsString],
) -> bool {
    let fixed_names = [
        Some(name.to_ascii_lowercase()),
        executable
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().to_ascii_lowercase()),
        command
            .first()
            .and_then(|value| Path::new(value).file_name())
            .map(|value| value.to_string_lossy().to_ascii_lowercase()),
    ];
    if fixed_names
        .into_iter()
        .flatten()
        .any(|value| value == "claude" || value == "claude.exe")
    {
        return true;
    }
    command
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .replace('\\', "/")
        .contains("@anthropic-ai/claude-code")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn binary_candidates_cover_linux_desktop_install_locations() {
        let home = tempfile::tempdir().expect("binary fixture home");
        let candidates = claude_binary_candidates(
            &["claude"],
            BinarySearchContext {
                search_path: None,
                home: Some(home.path()),
                data_dir: None,
                local_data_dir: None,
                unix_like: true,
                macos: false,
                windows: false,
            },
        );

        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/claude")));
        assert!(candidates.contains(&PathBuf::from("/usr/bin/claude")));
        assert!(candidates.contains(&home.path().join(".local/bin/claude")));
        assert!(candidates.contains(&home.path().join(".local/share/pnpm/claude")));
        assert!(candidates.contains(&home.path().join(".npm-global/bin/claude")));
    }

    #[test]
    fn windows_binary_and_process_shapes_cover_package_manager_wrappers() {
        let roaming = Path::new(r"C:\Users\CleanerX\AppData\Roaming");
        let local = Path::new(r"C:\Users\CleanerX\AppData\Local");
        let candidates = claude_binary_candidates(
            &["claude.exe", "claude.cmd", "claude.bat"],
            BinarySearchContext {
                search_path: None,
                home: None,
                data_dir: Some(roaming),
                local_data_dir: Some(local),
                unix_like: false,
                macos: false,
                windows: true,
            },
        );
        assert!(candidates.contains(&roaming.join("npm/claude.cmd")));
        assert!(candidates.contains(&local.join("pnpm/claude.bat")));
        assert!(is_claude_process_shape(
            "node.exe",
            Some(Path::new(r"C:\Program Files\nodejs\node.exe")),
            &[
                std::ffi::OsString::from("node.exe"),
                std::ffi::OsString::from(
                    r"C:\Users\CleanerX\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js",
                ),
            ],
        ));
    }

    #[tokio::test]
    async fn scans_recognized_sessions_memory_and_protected_data_without_retaining_bodies() {
        let fixture = tempfile::tempdir().expect("Claude fixture");
        let source = tempfile::tempdir().expect("source fixture");
        fs::write(source.path().join("protected-source.txt"), b"source-bytes")
            .expect("source fixture bytes");
        let bucket = fixture.path().join("projects/-tmp-project");
        fs::create_dir_all(bucket.join("memory")).expect("memory directory");
        fs::write(
            bucket.join("memory/MEMORY.md"),
            "# Project memory\nUse pnpm.",
        )
        .expect("memory");
        fs::write(fixture.path().join("settings.json"), "{\"theme\":\"dark\"}").expect("settings");
        write_transcript(&bucket.join(format!("{SESSION_ID}.jsonl")), source.path());

        let snapshot = ClaudeCodeAdapter::new()
            .scan(fixture.path().to_str())
            .await
            .expect("scan Claude fixture");

        assert_eq!(snapshot.installation.kind, AgentKind::ClaudeCode);
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].name, "Safe inventory title");
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(
            snapshot.projects[0].roots,
            vec![source.path().to_string_lossy()]
        );
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.category == StorageCategory::Memory)
        );
        assert!(snapshot.items.iter().any(|item| item.protected));
        assert!(
            !serde_json::to_string(&snapshot)
                .expect("serialize snapshot")
                .contains("PRIVATE_TRANSCRIPT_BODY")
        );
        assert_eq!(
            fs::read(source.path().join("protected-source.txt")).expect("source bytes"),
            b"source-bytes"
        );
    }

    #[tokio::test]
    async fn marks_every_mutable_item_blocked_while_a_writer_marker_exists() {
        let fixture = tempfile::tempdir().expect("Claude fixture");
        let bucket = fixture.path().join("projects/project");
        fs::create_dir_all(&bucket).expect("bucket");
        write_transcript(&bucket.join(format!("{SESSION_ID}.jsonl")), fixture.path());
        fs::create_dir_all(fixture.path().join("sessions")).expect("markers");
        fs::write(
            fixture
                .path()
                .join("sessions")
                .join(format!("{SESSION_ID}.json")),
            "{}",
        )
        .expect("marker");

        let snapshot = ClaudeCodeAdapter::new()
            .scan(fixture.path().to_str())
            .await
            .expect("scan active fixture");
        assert!(snapshot.installation.running);
        assert!(
            snapshot
                .items
                .iter()
                .filter(|item| !item.protected)
                .all(|item| item.blocked_reason.is_some())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinked_transcripts_without_following_them() {
        let fixture = tempfile::tempdir().expect("Claude fixture");
        let outside = tempfile::tempdir().expect("outside fixture");
        let bucket = fixture.path().join("projects/project");
        fs::create_dir_all(&bucket).expect("bucket");
        let outside_transcript = outside.path().join("outside.jsonl");
        write_transcript(&outside_transcript, outside.path());
        std::os::unix::fs::symlink(
            &outside_transcript,
            bucket.join(format!("{SESSION_ID}.jsonl")),
        )
        .expect("symlink");

        let snapshot = ClaudeCodeAdapter::new()
            .scan(fixture.path().to_str())
            .await
            .expect("safe scan");
        assert!(snapshot.sessions.is_empty());
        assert!(!snapshot.warnings.is_empty());
        assert!(outside_transcript.exists());
    }

    #[tokio::test]
    async fn loads_content_only_after_an_explicit_item_request() {
        let fixture = tempfile::tempdir().expect("Claude fixture");
        let bucket = fixture.path().join("projects/project");
        fs::create_dir_all(&bucket).expect("bucket");
        write_transcript(&bucket.join(format!("{SESSION_ID}.jsonl")), fixture.path());
        let adapter = ClaudeCodeAdapter::new();
        let snapshot = adapter.scan(fixture.path().to_str()).await.expect("scan");
        let item = snapshot
            .items
            .iter()
            .find(|item| item.thread_id.as_deref() == Some(SESSION_ID))
            .expect("session item");

        let detail = adapter
            .load_item_content(&snapshot.installation, item)
            .await
            .expect("detail");
        assert!(detail.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Message { text, .. } if text.contains("PRIVATE_TRANSCRIPT_BODY")
        )));
    }

    #[ignore = "requires a local Claude Code installation"]
    #[tokio::test]
    async fn live_scans_local_claude_metadata_without_mutation() {
        let adapter = ClaudeCodeAdapter::new();
        let snapshot = adapter.scan(None).await.expect("scan local Claude Code");
        assert_eq!(snapshot.installation.kind, AgentKind::ClaudeCode);
        assert!(
            snapshot
                .items
                .iter()
                .flat_map(|item| &item.paths)
                .all(|path| Path::new(path).starts_with(&snapshot.installation.home))
        );
    }

    fn write_transcript(path: &Path, cwd: &Path) {
        let cwd = serde_json::to_string(&cwd.to_string_lossy()).expect("cwd json");
        let contents = format!(
            "{{\"type\":\"user\",\"sessionId\":\"{SESSION_ID}\",\"session_id\":\"{SESSION_ID}\",\"cwd\":{cwd},\"entrypoint\":\"cli\",\"timestamp\":\"2026-08-26T10:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"PRIVATE_TRANSCRIPT_BODY\"}}}}\n{{\"type\":\"ai-title\",\"sessionId\":\"{SESSION_ID}\",\"aiTitle\":\"Safe inventory title\"}}\n"
        );
        fs::write(path, contents).expect("transcript");
    }
}
