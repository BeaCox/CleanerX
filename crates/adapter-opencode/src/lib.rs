//! OpenCode storage discovery and supported CLI session operations.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration as StdDuration, SystemTime};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentCapabilities, AgentInstallation, AgentKind, CategorySummary, CleanerError,
    CleanupItem, ContentBlock, InventorySnapshot, ItemContentDetail, ItemThumbnail,
    MemoryCapabilities, ProjectGroup, RiskLevel, SessionRecord, StorageCategory,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;
use sysinfo::System;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

const CONTENT_TEXT_LIMIT: usize = 512 * 1024;
const CONTENT_BLOCK_LIMIT: usize = 200;
const CONTENT_ROW_LIMIT: i64 = 64 * 1024;
const COMMAND_TIMEOUT: StdDuration = StdDuration::from_secs(120);

const PROTECTED_NAMES: &[&str] = &[
    "auth.json",
    "opencode.json",
    "opencode.jsonc",
    "storage",
    "project",
    "worktree",
    "repos",
    "plans",
    "plugin",
    "plugins",
    "skill",
    "skills",
    "command",
    "commands",
    "agent",
    "agents",
    "rules",
];

#[derive(Debug, Clone, Default)]
pub struct OpenCodeAdapter;

impl OpenCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    fn resolve_home(&self, custom_home: Option<&str>) -> Result<PathBuf, CleanerError> {
        let path = custom_home
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(default_data_home)
            .ok_or_else(|| CleanerError::NotFound("OpenCode data directory".into()))?;
        if !path.is_absolute() {
            return Err(CleanerError::InvalidRequest(
                "OpenCode data directory override must be an absolute path".into(),
            ));
        }
        Ok(path)
    }
}

#[async_trait]
impl AgentAdapter for OpenCodeAdapter {
    async fn detect(&self, custom_home: Option<&str>) -> Result<AgentInstallation, CleanerError> {
        let home = self.resolve_home(custom_home)?;
        let cache = default_cache_home();
        let binary = find_opencode_binary();
        let version = if let Some(binary) = &binary {
            let mut command = Command::new(binary);
            command.arg("--version").kill_on_drop(true);
            timeout(StdDuration::from_secs(10), command.output())
                .await
                .ok()
                .and_then(Result::ok)
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            None
        };
        let running = opencode_is_running();
        let mut warnings = Vec::new();
        if !home.exists() {
            warnings.push(format!(
                "OpenCode data directory does not exist: {}",
                home.display()
            ));
        }
        if binary.is_none() {
            warnings.push(
                "OpenCode executable was not found; session deletion and export are disabled"
                    .into(),
            );
        }

        let database = select_database(&home, &mut warnings);
        let recognized_database = database
            .as_deref()
            .and_then(|path| open_database(path).ok())
            .is_some_and(|connection| recognized_schema(&connection).is_ok());
        if database.is_some() && !recognized_database {
            warnings.push(
                "OpenCode database schema is not recognized; its contents remain read-only".into(),
            );
        }
        if running {
            warnings.push(
                "OpenCode is running; quit all OpenCode clients before deleting or restoring data"
                    .into(),
            );
        }

        let thread_delete = recognized_database && binary.is_some();
        Ok(AgentInstallation {
            kind: AgentKind::OpenCode,
            home: home.to_string_lossy().into_owned(),
            binary: binary.map(|path| path.to_string_lossy().into_owned()),
            version,
            app_support: cache.map(|path| path.to_string_lossy().into_owned()),
            running,
            capabilities: AgentCapabilities {
                thread_list: recognized_database,
                thread_delete,
                memory: MemoryCapabilities::default(),
                descendant_filter: recognized_database,
                report_only: !thread_delete,
            },
            warnings,
        })
    }

    async fn scan(&self, custom_home: Option<&str>) -> Result<InventorySnapshot, CleanerError> {
        let installation = self.detect(custom_home).await?;
        let home = PathBuf::from(&installation.home);
        let mut warnings = installation.warnings.clone();
        let mut sessions = Vec::new();
        let mut projects = Vec::new();
        let mut items = Vec::new();

        if let Some(database) = select_database(&home, &mut warnings) {
            match scan_database(&database, installation.running) {
                Ok((found_sessions, found_projects, found_items)) => {
                    sessions = found_sessions;
                    projects = found_projects;
                    items.extend(found_items);
                }
                Err(error) => warnings.push(format!(
                    "OpenCode session inventory is read-only because its database was not recognized: {error}"
                )),
            }
            add_protected_path(
                &database,
                "OpenCode session database",
                "OpenCode session and application state",
                &mut items,
                &mut warnings,
            );
            for suffix in ["-wal", "-shm"] {
                let sidecar = PathBuf::from(format!("{}{}", database.to_string_lossy(), suffix));
                if sidecar.exists() {
                    add_protected_path(
                        &sidecar,
                        &format!("OpenCode database {suffix}"),
                        "OpenCode live database state",
                        &mut items,
                        &mut warnings,
                    );
                }
            }
        }

        scan_logs(&home, installation.running, &mut items, &mut warnings);
        if let Some(cache) = installation.app_support.as_deref().map(PathBuf::from) {
            scan_cache(&cache, installation.running, &mut items, &mut warnings);
        }
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
        if installation.kind != AgentKind::OpenCode {
            return Err(CleanerError::InvalidRequest(
                "OpenCode content request used a different Agent installation".into(),
            ));
        }
        if item.protected || item.category == StorageCategory::Protected {
            return Ok(content_notice(
                item,
                "protected",
                "CleanerX never opens OpenCode authentication, configuration, plugins, skills, worktrees, or raw database files.",
            ));
        }
        match item.category {
            StorageCategory::Session | StorageCategory::ArchivedSession => {
                content_from_database(installation, item)
            }
            StorageCategory::Log => content_from_logs(installation, item),
            _ => Ok(content_notice(
                item,
                "filesystem.metadataOnly",
                "This recognized OpenCode application-data item is inventoried by metadata only.",
            )),
        }
    }

    async fn load_item_thumbnail(
        &self,
        _installation: &AgentInstallation,
        _item: &CleanupItem,
    ) -> Result<Option<ItemThumbnail>, CleanerError> {
        Err(CleanerError::Unsupported(
            "OpenCode inventory does not expose thumbnail items".into(),
        ))
    }

    async fn delete_sessions(
        &self,
        installation: &AgentInstallation,
        session_ids: &[String],
    ) -> Result<Vec<String>, CleanerError> {
        validate_offline_installation(installation)?;
        let binary = installation
            .binary
            .as_deref()
            .ok_or_else(|| CleanerError::NotFound("OpenCode executable".into()))?;
        let database = database_for_installation(installation)?;
        let connection = open_database(&database)?;
        recognized_schema(&connection)?;
        for session_id in session_ids {
            validate_session_id(session_id)?;
            if !session_exists(&connection, session_id)? {
                return Err(CleanerError::NotFound(format!(
                    "OpenCode session {session_id}"
                )));
            }
        }
        drop(connection);

        let mut deleted = Vec::new();
        for session_id in session_ids {
            run_delete(binary, &database, session_id).await?;
            deleted.push(session_id.clone());
        }
        Ok(deleted)
    }

    async fn reset_memory(&self, _installation: &AgentInstallation) -> Result<(), CleanerError> {
        Err(CleanerError::Unsupported(
            "OpenCode does not expose a supported memory reset capability".into(),
        ))
    }

    async fn export_sessions(
        &self,
        installation: &AgentInstallation,
        session_ids: &[String],
        destination: &Path,
    ) -> Result<Vec<PathBuf>, CleanerError> {
        validate_offline_installation(installation)?;
        let binary = installation
            .binary
            .as_deref()
            .ok_or_else(|| CleanerError::NotFound("OpenCode executable".into()))?;
        let database = database_for_installation(installation)?;
        let connection = open_database(&database)?;
        recognized_schema(&connection)?;
        if !destination.is_absolute() {
            return Err(CleanerError::UnsafePath(destination.display().to_string()));
        }
        fs::create_dir_all(destination)?;
        if fs::symlink_metadata(destination)?.file_type().is_symlink() {
            return Err(CleanerError::UnsafePath(destination.display().to_string()));
        }
        for session_id in session_ids {
            validate_session_id(session_id)?;
            if !session_exists(&connection, session_id)? {
                return Err(CleanerError::NotFound(format!(
                    "OpenCode session {session_id}"
                )));
            }
        }
        drop(connection);

        let mut exports = Vec::new();
        for session_id in session_ids {
            let path = destination.join(format!("{session_id}.json"));
            let file = create_private_file(&path)?;
            let mut command = supported_command(binary, &database);
            command
                .arg("export")
                .arg(session_id)
                .current_dir(&installation.home)
                .stdout(Stdio::from(file))
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let child = command.spawn().map_err(CleanerError::Io)?;
            let output = timeout(COMMAND_TIMEOUT, child.wait_with_output())
                .await
                .map_err(|_| {
                    CleanerError::Integration("OpenCode session export timed out".into())
                })??;
            if !output.status.success() {
                let _ = fs::remove_file(&path);
                return Err(CleanerError::Integration(format!(
                    "OpenCode session export failed: {}",
                    bounded_stderr(&output.stderr)
                )));
            }
            let header = read_export_header(&path)?;
            if header.info.id != *session_id {
                let _ = fs::remove_file(&path);
                return Err(CleanerError::Integration(format!(
                    "OpenCode exported an unexpected session for {session_id}"
                )));
            }
            exports.push(path);
        }
        Ok(exports)
    }

    async fn import_sessions(
        &self,
        installation: &AgentInstallation,
        exports: &[PathBuf],
    ) -> Result<Vec<String>, CleanerError> {
        validate_offline_installation(installation)?;
        let binary = installation
            .binary
            .as_deref()
            .ok_or_else(|| CleanerError::NotFound("OpenCode executable".into()))?;
        let database = database_for_installation(installation)?;
        let connection = open_database(&database)?;
        recognized_schema(&connection)?;

        let mut headers = Vec::new();
        let mut seen = HashSet::new();
        for path in exports {
            if !is_plain_file(path)? {
                return Err(CleanerError::UnsafePath(path.display().to_string()));
            }
            let header = read_export_header(path)?;
            validate_session_id(&header.info.id)?;
            if !seen.insert(header.info.id.clone()) {
                return Err(CleanerError::InvalidRequest(format!(
                    "duplicate OpenCode export ID: {}",
                    header.info.id
                )));
            }
            if session_exists(&connection, &header.info.id)? {
                return Err(CleanerError::Blocked(format!(
                    "OpenCode session already exists: {}",
                    header.info.id
                )));
            }
            let directory = PathBuf::from(&header.info.directory);
            if !directory.is_absolute() || !directory.is_dir() {
                return Err(CleanerError::Blocked(format!(
                    "OpenCode session working directory is unavailable: {}",
                    directory.display()
                )));
            }
            headers.push((path.clone(), header.info.id, directory));
        }
        drop(connection);

        let mut imported = Vec::new();
        for (path, session_id, directory) in &headers {
            let mut command = supported_command(binary, &database);
            command
                .arg("import")
                .arg(path)
                .current_dir(directory)
                .kill_on_drop(true);
            let result = timeout(COMMAND_TIMEOUT, command.output()).await;
            let failure = match result {
                Ok(Ok(output)) if output.status.success() => None,
                Ok(Ok(output)) => Some(format!(
                    "OpenCode session import failed: {}",
                    bounded_stderr(&output.stderr)
                )),
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("OpenCode session import timed out".into()),
            };
            if let Some(failure) = failure {
                if open_database(&database)
                    .and_then(|connection| session_exists(&connection, session_id))
                    .unwrap_or(false)
                {
                    imported.push(session_id.clone());
                }
                let rollback = rollback_imports(binary, &database, &imported).await;
                return Err(CleanerError::Integration(match rollback {
                    Ok(()) => failure,
                    Err(error) => format!("{failure}; rollback also failed: {error}"),
                }));
            }
            let connection = open_database(&database)?;
            if !session_exists(&connection, session_id)? {
                drop(connection);
                let rollback = rollback_imports(binary, &database, &imported).await;
                return Err(CleanerError::Integration(match rollback {
                    Ok(()) => format!("OpenCode did not restore session {session_id}"),
                    Err(error) => format!(
                        "OpenCode did not restore session {session_id}; rollback also failed: {error}"
                    ),
                }));
            }
            imported.push(session_id.clone());
        }
        Ok(imported)
    }
}

fn default_data_home() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))
        .map(|root| root.join("opencode"))
}

fn default_cache_home() -> Option<PathBuf> {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))
        .map(|root| root.join("opencode"))
        .filter(|path| path.is_absolute())
}

fn select_database(home: &Path, warnings: &mut Vec<String>) -> Option<PathBuf> {
    if !home.is_dir() {
        return None;
    }
    let primary = home.join("opencode.db");
    if is_plain_file(&primary).ok() == Some(true) {
        return Some(primary);
    }
    let mut candidates = fs::read_dir(home)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("opencode-") && name.ends_with(".db"))
                && is_plain_file(path).ok() == Some(true)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    match candidates.as_slice() {
        [database] => Some(database.clone()),
        [] => None,
        _ => {
            warnings.push(format!(
                "Multiple OpenCode channel databases were found under {}; select a custom data directory containing one active database",
                home.display()
            ));
            None
        }
    }
}

fn database_for_installation(installation: &AgentInstallation) -> Result<PathBuf, CleanerError> {
    let mut warnings = Vec::new();
    select_database(Path::new(&installation.home), &mut warnings).ok_or_else(|| {
        CleanerError::NotFound(if warnings.is_empty() {
            "recognized OpenCode database".into()
        } else {
            warnings.join("; ")
        })
    })
}

fn open_database(path: &Path) -> Result<Connection, CleanerError> {
    if !is_plain_file(path)? {
        return Err(CleanerError::UnsafePath(path.display().to_string()));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    connection.busy_timeout(StdDuration::from_secs(2))?;
    Ok(connection)
}

fn recognized_schema(connection: &Connection) -> Result<(), CleanerError> {
    require_columns(
        connection,
        "session",
        &[
            "id",
            "project_id",
            "parent_id",
            "directory",
            "title",
            "time_created",
            "time_updated",
            "time_archived",
        ],
    )?;
    require_columns(connection, "project", &["id", "worktree", "name"])?;
    Ok(())
}

fn require_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<(), CleanerError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if required.iter().all(|column| columns.contains(*column)) {
        return Ok(());
    }
    Err(CleanerError::Unsupported(format!(
        "unrecognized OpenCode {table} table"
    )))
}

fn table_has_columns(connection: &Connection, table: &str, required: &[&str]) -> bool {
    require_columns(connection, table, required).is_ok()
}

#[derive(Debug)]
struct DatabaseSession {
    id: String,
    title: String,
    directory: String,
    parent_id: Option<String>,
    project_id: String,
    project_name: Option<String>,
    worktree: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    archived: bool,
    size_bytes: u64,
}

type DatabaseInventory = (Vec<SessionRecord>, Vec<ProjectGroup>, Vec<CleanupItem>);

fn scan_database(database: &Path, running: bool) -> Result<DatabaseInventory, CleanerError> {
    let connection = open_database(database)?;
    recognized_schema(&connection)?;
    let revisions = database_revision_paths(database);
    let (source_revision, safety_blocker) = match cleanerx_core::metadata_revision(&revisions) {
        Ok(revision) => (Some(revision), None),
        Err(error) => (
            None,
            Some(format!(
                "OpenCode database failed a filesystem safety check: {error}"
            )),
        ),
    };
    let sizes = session_sizes(&connection)?;
    let mut statement = connection.prepare(
        "SELECT s.id, s.title, s.directory, s.parent_id, s.project_id, p.name, p.worktree, \
                s.time_created, s.time_updated, s.time_archived \
         FROM session s LEFT JOIN project p ON p.id = s.project_id \
         ORDER BY s.time_updated DESC, s.id DESC",
    )?;
    let records = statement
        .query_map([], |row| {
            let created: Option<i64> = row.get(7)?;
            let updated: Option<i64> = row.get(8)?;
            let archived: Option<i64> = row.get(9)?;
            let id: String = row.get(0)?;
            Ok(DatabaseSession {
                size_bytes: sizes.get(&id).copied().unwrap_or(0),
                id,
                title: row.get(1)?,
                directory: row.get(2)?,
                parent_id: row.get(3)?,
                project_id: row.get(4)?,
                project_name: row.get(5)?,
                worktree: row.get(6)?,
                created_at: created.and_then(DateTime::<Utc>::from_timestamp_millis),
                updated_at: updated.and_then(DateTime::<Utc>::from_timestamp_millis),
                archived: archived.is_some(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let child_map = records.iter().fold(
        HashMap::<String, Vec<String>>::new(),
        |mut children, session| {
            if let Some(parent) = &session.parent_id {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(session.id.clone());
            }
            children
        },
    );
    let ids = records
        .iter()
        .map(|session| session.id.clone())
        .collect::<HashSet<_>>();
    let blocker = running
        .then(|| "OpenCode is running; quit it before deleting session data".into())
        .or(safety_blocker);
    let mut sessions = Vec::new();
    let mut items = Vec::new();
    for session in &records {
        let descendants = descendants(&session.id, &child_map, &ids);
        let name = if session.title.trim().is_empty() {
            format!("OpenCode session {}", short_id(&session.id))
        } else {
            session.title.clone()
        };
        sessions.push(SessionRecord {
            id: session.id.clone(),
            name: name.clone(),
            cwd: session.directory.clone(),
            path: None,
            source: "cli".into(),
            archived: session.archived,
            pinned: false,
            status: if running { "loaded" } else { "notLoaded" }.into(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            size_bytes: session.size_bytes,
            parent_thread_id: session.parent_id.clone(),
            descendant_ids: descendants,
        });
        let mut metadata = BTreeMap::from([
            (
                "databasePath".into(),
                database.to_string_lossy().into_owned(),
            ),
            ("source".into(), "official OpenCode SQLite schema".into()),
            ("pinned".into(), "false".into()),
        ]);
        if let Some(revision) = &source_revision {
            metadata.insert("sourceRevision".into(), revision.clone());
        }
        items.push(CleanupItem {
            id: format!("session:{}", session.id),
            category: if session.archived {
                StorageCategory::ArchivedSession
            } else {
                StorageCategory::Session
            },
            title: name,
            subtitle: (!session.directory.is_empty()).then(|| session.directory.clone()),
            paths: Vec::new(),
            project_id: Some(session.project_id.clone()),
            thread_id: Some(session.id.clone()),
            size_bytes: session.size_bytes,
            modified_at: session.updated_at,
            risk: RiskLevel::High,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason: blocker.clone(),
            metadata,
        });
    }

    let mut projects_by_id = BTreeMap::<String, ProjectGroup>::new();
    for session in &records {
        let entry = projects_by_id
            .entry(session.project_id.clone())
            .or_insert_with(|| ProjectGroup {
                id: session.project_id.clone(),
                name: session
                    .project_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .or_else(|| project_name(session.worktree.as_deref()))
                    .unwrap_or_else(|| "OpenCode project".into()),
                roots: session
                    .worktree
                    .clone()
                    .filter(|root| !root.trim().is_empty())
                    .into_iter()
                    .collect(),
                session_ids: Vec::new(),
                size_bytes: 0,
            });
        entry.session_ids.push(session.id.clone());
        entry.size_bytes = entry.size_bytes.saturating_add(session.size_bytes);
    }
    Ok((sessions, projects_by_id.into_values().collect(), items))
}

fn session_sizes(connection: &Connection) -> Result<HashMap<String, u64>, CleanerError> {
    let mut sizes = HashMap::new();
    add_grouped_sizes(
        connection,
        "SELECT id, length(id) + length(title) + length(directory) FROM session",
        &mut sizes,
    )?;
    for (table, id_column, value_expression, required) in [
        (
            "message",
            "session_id",
            "length(data)",
            &["session_id", "data"][..],
        ),
        (
            "part",
            "session_id",
            "length(data)",
            &["session_id", "data"][..],
        ),
        (
            "session_message",
            "session_id",
            "length(data)",
            &["session_id", "data"][..],
        ),
        (
            "session_input",
            "session_id",
            "length(prompt)",
            &["session_id", "prompt"][..],
        ),
        (
            "session_context_epoch",
            "session_id",
            "length(baseline) + length(snapshot)",
            &["session_id", "baseline", "snapshot"][..],
        ),
        (
            "todo",
            "session_id",
            "length(content) + length(status)",
            &["session_id", "content", "status"][..],
        ),
    ] {
        if table_has_columns(connection, table, required) {
            add_grouped_sizes(
                connection,
                &format!(
                    "SELECT {id_column}, COALESCE(SUM({value_expression}), 0) FROM {table} GROUP BY {id_column}"
                ),
                &mut sizes,
            )?;
        }
    }
    if table_has_columns(connection, "event", &["aggregate_id", "data", "type"]) {
        add_grouped_sizes(
            connection,
            "SELECT aggregate_id, COALESCE(SUM(length(data) + length(type)), 0) FROM event GROUP BY aggregate_id",
            &mut sizes,
        )?;
    }
    Ok(sizes)
}

fn add_grouped_sizes(
    connection: &Connection,
    query: &str,
    sizes: &mut HashMap<String, u64>,
) -> Result<(), CleanerError> {
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (session_id, bytes) = row?;
        let bytes = u64::try_from(bytes.max(0)).unwrap_or_default();
        *sizes.entry(session_id).or_default() = sizes
            .get(&session_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(bytes);
    }
    Ok(())
}

fn descendants(
    root: &str,
    children: &HashMap<String, Vec<String>>,
    known: &HashSet<String>,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut pending = children.get(root).cloned().unwrap_or_default();
    let mut seen = HashSet::from([root.to_owned()]);
    while let Some(id) = pending.pop() {
        if !known.contains(&id) || !seen.insert(id.clone()) {
            continue;
        }
        result.push(id.clone());
        if let Some(next) = children.get(&id) {
            pending.extend(next.iter().cloned());
        }
    }
    result.sort();
    result
}

fn scan_logs(home: &Path, running: bool, items: &mut Vec<CleanupItem>, warnings: &mut Vec<String>) {
    let path = home.join("log");
    if !path.exists() {
        return;
    }
    add_mutable_directory_item(
        &path,
        "logs:opencode",
        StorageCategory::Log,
        "OpenCode diagnostic logs",
        "Recognized OpenCode log directory",
        RiskLevel::Review,
        false,
        running,
        items,
        warnings,
    );
}

fn scan_cache(
    cache: &Path,
    running: bool,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) {
    if !cache.exists() {
        return;
    }
    add_mutable_directory_item(
        cache,
        "cache:opencode",
        StorageCategory::Cache,
        "OpenCode caches",
        "Regenerable model, package, and tool caches",
        RiskLevel::Safe,
        false,
        running,
        items,
        warnings,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_mutable_directory_item(
    path: &Path,
    id: &str,
    category: StorageCategory,
    title: &str,
    subtitle: &str,
    risk: RiskLevel,
    recoverable: bool,
    running: bool,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) {
    if is_plain_directory(path).ok() != Some(true) {
        warnings.push(format!(
            "Skipped linked or unrecognized OpenCode directory: {}",
            path.display()
        ));
        return;
    }
    let size_bytes = match cleanerx_core::safety::allocated_size(path) {
        Ok(size) => size,
        Err(error) => {
            warnings.push(format!(
                "Could not size OpenCode data at {}: {error}",
                path.display()
            ));
            return;
        }
    };
    let safety = cleanerx_core::metadata_revision(&[path.to_path_buf()]);
    let blocked_reason = running
        .then(|| "OpenCode is running; quit it before cleaning writable application data".into())
        .or_else(|| {
            safety
                .as_ref()
                .err()
                .map(|error| format!("OpenCode data failed a filesystem safety check: {error}"))
        });
    let mut metadata = BTreeMap::new();
    if let Ok(revision) = safety {
        metadata.insert("sourceRevision".into(), revision);
    }
    items.push(CleanupItem {
        id: id.into(),
        category,
        title: title.into(),
        subtitle: Some(subtitle.into()),
        paths: vec![path.to_string_lossy().into_owned()],
        project_id: None,
        thread_id: None,
        size_bytes,
        modified_at: modified_at(path),
        risk,
        recoverable,
        default_selected: false,
        protected: false,
        blocked_reason,
        metadata,
    });
}

fn scan_protected(home: &Path, items: &mut Vec<CleanupItem>, warnings: &mut Vec<String>) {
    for name in PROTECTED_NAMES {
        let path = home.join(name);
        if path.exists() {
            add_protected_path(
                &path,
                name,
                "OpenCode authentication, configuration, extension, or source-managed data",
                items,
                warnings,
            );
        }
    }
}

fn add_protected_path(
    path: &Path,
    title: &str,
    subtitle: &str,
    items: &mut Vec<CleanupItem>,
    warnings: &mut Vec<String>,
) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            warnings.push(format!(
                "Could not inspect protected OpenCode path {}: {error}",
                path.display()
            ));
            return;
        }
    };
    let size_bytes = if metadata.file_type().is_symlink() {
        warnings.push(format!(
            "Protected OpenCode path is a symbolic link and was not followed: {}",
            path.display()
        ));
        0
    } else {
        cleanerx_core::safety::allocated_size(path).unwrap_or_default()
    };
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, path.to_string_lossy().as_bytes());
    items.push(CleanupItem {
        id: format!("protected:opencode:{id}"),
        category: StorageCategory::Protected,
        title: title.into(),
        subtitle: Some(subtitle.into()),
        paths: vec![path.to_string_lossy().into_owned()],
        project_id: None,
        thread_id: None,
        size_bytes,
        modified_at: modified_at(path),
        risk: RiskLevel::Protected,
        recoverable: false,
        default_selected: false,
        protected: true,
        blocked_reason: Some("Protected OpenCode data".into()),
        metadata: BTreeMap::new(),
    });
}

fn content_from_database(
    installation: &AgentInstallation,
    item: &CleanupItem,
) -> Result<ItemContentDetail, CleanerError> {
    let session_id = item
        .thread_id
        .as_deref()
        .ok_or_else(|| CleanerError::InvalidRequest("OpenCode session item has no ID".into()))?;
    validate_session_id(session_id)?;
    let database = database_for_installation(installation)?;
    let expected = item
        .metadata
        .get("databasePath")
        .ok_or_else(|| CleanerError::InvalidRequest("OpenCode database path is missing".into()))?;
    if Path::new(expected) != database {
        return Err(CleanerError::UnsafePath(expected.clone()));
    }
    let connection = open_database(&database)?;
    recognized_schema(&connection)?;
    if !table_has_columns(&connection, "message", &["id", "session_id", "data"])
        || !table_has_columns(&connection, "part", &["message_id", "session_id", "data"])
    {
        return Ok(content_notice(
            item,
            "opencodeDb.readOnly",
            "This recognized OpenCode schema does not expose the legacy message projection used by the bounded preview.",
        ));
    }

    let mut statement = connection.prepare(
        "SELECT substr(m.data, 1, ?2), substr(p.data, 1, ?2), length(m.data), length(p.data) \
         FROM message m JOIN part p ON p.message_id = m.id \
         WHERE m.session_id = ?1 AND p.session_id = ?1 \
         ORDER BY m.time_created, m.id, p.id LIMIT ?3",
    )?;
    let mut rows = statement.query(params![
        session_id,
        CONTENT_ROW_LIMIT,
        i64::try_from(CONTENT_BLOCK_LIMIT + 1).unwrap_or(201)
    ])?;
    let mut blocks = Vec::new();
    let mut bytes_read = 0_usize;
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if blocks.len() >= CONTENT_BLOCK_LIMIT || bytes_read >= CONTENT_TEXT_LIMIT {
            truncated = true;
            break;
        }
        let message_data: String = row.get(0)?;
        let part_data: String = row.get(1)?;
        let message_len: i64 = row.get(2)?;
        let part_len: i64 = row.get(3)?;
        bytes_read = bytes_read
            .saturating_add(message_data.len())
            .saturating_add(part_data.len());
        if message_len > CONTENT_ROW_LIMIT || part_len > CONTENT_ROW_LIMIT {
            truncated = true;
        }
        let message = serde_json::from_str::<Value>(&message_data).ok();
        let part = serde_json::from_str::<Value>(&part_data).ok();
        let role = message
            .as_ref()
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("assistant")
            .to_owned();
        let Some(part) = part else {
            continue;
        };
        match part.get("type").and_then(Value::as_str) {
            Some("text" | "reasoning") => {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    blocks.push(ContentBlock::Message {
                        role,
                        text: bounded_string(
                            text.to_owned(),
                            CONTENT_TEXT_LIMIT - bytes_read.min(CONTENT_TEXT_LIMIT),
                        ),
                        phase: None,
                    });
                }
            }
            Some("tool") => {
                let state = part.get("state");
                let title = state
                    .and_then(|value| value.get("title"))
                    .and_then(Value::as_str)
                    .or_else(|| part.get("tool").and_then(Value::as_str))
                    .unwrap_or("Tool");
                let text = state
                    .and_then(|value| value.get("output"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                blocks.push(ContentBlock::Text {
                    title: bounded_string(title.to_owned(), 256),
                    text: bounded_string(text.to_owned(), 16 * 1024),
                });
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        blocks.push(ContentBlock::Notice {
            text: "This OpenCode session has no previewable text projection.".into(),
        });
    }
    Ok(ItemContentDetail {
        item_id: item.id.clone(),
        source: "opencodeDb.readOnly".into(),
        truncated,
        bytes_read: u64::try_from(bytes_read.min(CONTENT_TEXT_LIMIT)).unwrap_or_default(),
        blocks,
        warning: Some(
            "Read-only preview from a recognized OpenCode SQLite schema; the database is never modified directly."
                .into(),
        ),
    })
}

fn content_from_logs(
    installation: &AgentInstallation,
    item: &CleanupItem,
) -> Result<ItemContentDetail, CleanerError> {
    let home = PathBuf::from(&installation.home).canonicalize()?;
    let path = item
        .paths
        .first()
        .map(PathBuf::from)
        .ok_or_else(|| CleanerError::NotFound("OpenCode log directory".into()))?;
    if !is_plain_directory(&path)? || !path.canonicalize()?.starts_with(&home) {
        return Err(CleanerError::UnsafePath(path.display().to_string()));
    }
    let mut files = fs::read_dir(&path)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_plain_file(path).ok() == Some(true))
        .collect::<Vec<_>>();
    files.sort_by_key(|path| std::cmp::Reverse(modified_at(path)));
    let mut blocks = Vec::new();
    let mut bytes_read = 0_usize;
    let mut truncated = false;
    for file in files {
        if blocks.len() >= CONTENT_BLOCK_LIMIT || bytes_read >= CONTENT_TEXT_LIMIT {
            truncated = true;
            break;
        }
        let reader = BufReader::new(fs::File::open(&file)?);
        for line in reader.lines() {
            let line = line?;
            bytes_read = bytes_read.saturating_add(line.len());
            if blocks.len() >= CONTENT_BLOCK_LIMIT || bytes_read > CONTENT_TEXT_LIMIT {
                truncated = true;
                break;
            }
            blocks.push(ContentBlock::Log {
                timestamp: None,
                level: None,
                target: file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                text: line,
            });
        }
    }
    if blocks.is_empty() {
        blocks.push(ContentBlock::Notice {
            text: "The OpenCode log directory contains no previewable lines.".into(),
        });
    }
    Ok(ItemContentDetail {
        item_id: item.id.clone(),
        source: "opencodeLogs.readOnly".into(),
        truncated,
        bytes_read: u64::try_from(bytes_read.min(CONTENT_TEXT_LIMIT)).unwrap_or_default(),
        blocks,
        warning: Some("OpenCode logs can contain prompts, file paths, and tool output.".into()),
    })
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

fn validate_offline_installation(installation: &AgentInstallation) -> Result<(), CleanerError> {
    if installation.kind != AgentKind::OpenCode {
        return Err(CleanerError::InvalidRequest(
            "OpenCode operation used a different Agent installation".into(),
        ));
    }
    if installation.running || opencode_is_running() {
        return Err(CleanerError::Blocked(
            "Quit all OpenCode clients before changing session data".into(),
        ));
    }
    Ok(())
}

async fn run_delete(binary: &str, database: &Path, session_id: &str) -> Result<(), CleanerError> {
    let mut command = supported_command(binary, database);
    command
        .arg("session")
        .arg("delete")
        .arg(session_id)
        .current_dir(database.parent().unwrap_or_else(|| Path::new("/")))
        .kill_on_drop(true);
    let output = timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| CleanerError::Integration("OpenCode session deletion timed out".into()))??;
    if !output.status.success() {
        return Err(CleanerError::Integration(format!(
            "OpenCode session deletion failed: {}",
            bounded_stderr(&output.stderr)
        )));
    }
    Ok(())
}

async fn rollback_imports(
    binary: &str,
    database: &Path,
    imported: &[String],
) -> Result<(), CleanerError> {
    for session_id in imported.iter().rev() {
        run_delete(binary, database, session_id).await?;
    }
    Ok(())
}

fn supported_command(binary: &str, database: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("--pure")
        .env("OPENCODE_DB", database)
        .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
        .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");
    command
}

#[derive(Debug, Deserialize)]
struct ExportEnvelope {
    info: ExportInfo,
    messages: IgnoredAny,
}

#[derive(Debug, Deserialize)]
struct ExportInfo {
    id: String,
    directory: String,
}

fn read_export_header(path: &Path) -> Result<ExportEnvelope, CleanerError> {
    let file = fs::File::open(path)?;
    let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(file));
    let envelope = ExportEnvelope::deserialize(&mut deserializer)?;
    deserializer.end()?;
    let _ = &envelope.messages;
    Ok(envelope)
}

fn create_private_file(path: &Path) -> Result<fs::File, CleanerError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn validate_session_id(session_id: &str) -> Result<(), CleanerError> {
    if session_id.len() > 128
        || !session_id.starts_with("ses_")
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(CleanerError::InvalidRequest(format!(
            "invalid OpenCode session ID: {session_id}"
        )));
    }
    Ok(())
}

fn session_exists(connection: &Connection, session_id: &str) -> Result<bool, CleanerError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM session WHERE id = ?1 LIMIT 1",
            [session_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn database_revision_paths(database: &Path) -> Vec<PathBuf> {
    let mut paths = vec![database.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{}", database.to_string_lossy(), suffix));
        if path.exists() {
            paths.push(path);
        }
    }
    paths
}

fn bounded_stderr(stderr: &[u8]) -> String {
    bounded_string(String::from_utf8_lossy(stderr).trim().to_owned(), 4096)
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
    value.push('…');
    value
}

fn project_name(worktree: Option<&str>) -> Option<String> {
    let worktree = worktree?;
    if worktree == "/" {
        return Some("Global".into());
    }
    Path::new(worktree)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

fn short_id(id: &str) -> &str {
    id.get(id.len().saturating_sub(8)..).unwrap_or(id)
}

fn summarize_categories(items: &[CleanupItem]) -> Vec<CategorySummary> {
    let mut summaries = BTreeMap::<StorageCategory, CategorySummary>::new();
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

fn find_opencode_binary() -> Option<PathBuf> {
    let executable_name = if cfg!(windows) {
        "opencode.exe"
    } else {
        "opencode"
    };
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|directory| directory.join(executable_name)));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/opencode"),
        PathBuf::from("/usr/local/bin/opencode"),
    ]);
    if let Some(home) = dirs::home_dir() {
        candidates.extend([
            home.join(".local/bin").join(executable_name),
            home.join(".opencode/bin").join(executable_name),
            home.join(".volta/bin").join(executable_name),
            home.join(".asdf/shims").join(executable_name),
            home.join(".bun/bin").join(executable_name),
            home.join("Library/pnpm").join(executable_name),
        ]);
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut nvm_candidates = entries
                .flatten()
                .map(|entry| entry.path().join("bin").join(executable_name))
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
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

fn opencode_is_running() -> bool {
    let system = System::new_all();
    system.processes().values().any(|process| {
        let names = [
            Some(process.name().to_string_lossy().to_ascii_lowercase()),
            process
                .exe()
                .and_then(Path::file_name)
                .map(|value| value.to_string_lossy().to_ascii_lowercase()),
            process
                .cmd()
                .first()
                .and_then(|value| Path::new(value).file_name())
                .map(|value| value.to_string_lossy().to_ascii_lowercase()),
        ];
        names.into_iter().flatten().any(|name| {
            matches!(
                name.as_str(),
                "opencode" | "opencode.exe" | "opencode2" | "opencode2.exe"
            ) || name.starts_with("opencode-")
                || name.starts_with("opencode2-")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_ID: &str = "ses_root111111111111111111111";
    const CHILD_ID: &str = "ses_child2222222222222222222";

    #[tokio::test]
    async fn scans_recognized_database_without_retaining_transcript_bodies() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let home = fixture.path().join("opencode");
        fs::create_dir_all(home.join("log")).expect("data directories");
        fs::write(home.join("auth.json"), b"top-secret-api-key").expect("auth fixture");
        fs::write(home.join("log/session.log"), b"private-log-body").expect("log fixture");
        create_database(&home.join("opencode.db"));

        let snapshot = OpenCodeAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan fixture");

        assert_eq!(snapshot.installation.kind, AgentKind::OpenCode);
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(
            snapshot
                .sessions
                .iter()
                .find(|session| session.id == ROOT_ID)
                .expect("root session")
                .descendant_ids,
            vec![CHILD_ID]
        );
        assert!(snapshot.items.iter().any(|item| item.protected));
        let serialized = serde_json::to_string(&snapshot).expect("snapshot JSON");
        assert!(!serialized.contains("private transcript body"));
        assert!(!serialized.contains("top-secret-api-key"));
        assert!(!serialized.contains("private-log-body"));
    }

    #[tokio::test]
    async fn unknown_database_schema_does_not_block_filesystem_inventory() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let home = fixture.path().join("opencode");
        fs::create_dir_all(home.join("log")).expect("log directory");
        fs::write(home.join("log/opencode.log"), b"metadata only").expect("log");
        Connection::open(home.join("opencode.db"))
            .expect("database")
            .execute("CREATE TABLE mystery (id TEXT)", [])
            .expect("unknown schema");

        let snapshot = OpenCodeAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan fixture");

        assert!(snapshot.sessions.is_empty());
        assert!(snapshot.installation.capabilities.report_only);
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.category == StorageCategory::Log)
        );
    }

    #[tokio::test]
    async fn reads_content_only_after_a_bounded_detail_request() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let home = fixture.path().join("opencode");
        fs::create_dir_all(&home).expect("data directory");
        create_database(&home.join("opencode.db"));
        let adapter = OpenCodeAdapter::new();
        let snapshot = adapter.scan(home.to_str()).await.expect("scan");
        let item = snapshot
            .items
            .iter()
            .find(|item| item.thread_id.as_deref() == Some(ROOT_ID))
            .expect("root item");

        let detail = adapter
            .load_item_content(&snapshot.installation, item)
            .await
            .expect("detail");

        assert_eq!(detail.source, "opencodeDb.readOnly");
        assert!(detail.bytes_read <= CONTENT_TEXT_LIMIT as u64);
        assert!(detail.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Message { text, .. } if text.contains("private transcript body")
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_cache_is_never_a_cleanup_target() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let home = fixture.path().join("opencode");
        let outside = fixture.path().join("outside");
        let cache_root = fixture.path().join("cache-root");
        fs::create_dir_all(&home).expect("data directory");
        fs::create_dir_all(&outside).expect("outside directory");
        fs::create_dir_all(&cache_root).expect("cache parent");
        fs::write(outside.join("private"), b"protected bytes").expect("outside bytes");
        std::os::unix::fs::symlink(&outside, cache_root.join("opencode")).expect("cache link");
        create_database(&home.join("opencode.db"));

        let old_cache = env::var_os("XDG_CACHE_HOME");
        // SAFETY: this test restores the process environment before returning.
        unsafe { env::set_var("XDG_CACHE_HOME", &cache_root) };
        let snapshot = OpenCodeAdapter::new()
            .scan(home.to_str())
            .await
            .expect("scan");
        match old_cache {
            Some(value) => unsafe { env::set_var("XDG_CACHE_HOME", value) },
            None => unsafe { env::remove_var("XDG_CACHE_HOME") },
        }

        assert!(
            !snapshot
                .items
                .iter()
                .any(|item| item.id == "cache:opencode")
        );
        assert_eq!(
            fs::read(outside.join("private")).expect("outside remains"),
            b"protected bytes"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_deletion_uses_the_official_cli_command_shape() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().expect("CLI fixture");
        let binary = fixture.path().join("opencode");
        let database = fixture.path().join("opencode.db");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"${OPENCODE_DB}.args\"\n",
        )
        .expect("mock binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");

        run_delete(binary.to_str().expect("binary path"), &database, ROOT_ID)
            .await
            .expect("official delete route");

        let args = fs::read_to_string(database.with_extension("db.args")).expect("captured args");
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["--pure", "session", "delete", ROOT_ID]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_official_deletion_is_reported_without_a_database_fallback() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().expect("CLI fixture");
        let binary = fixture.path().join("opencode");
        let database = fixture.path().join("opencode.db");
        fs::write(&binary, "#!/bin/sh\necho 'refused' >&2\nexit 23\n").expect("mock binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");

        let error = run_delete(binary.to_str().expect("binary path"), &database, ROOT_ID)
            .await
            .expect_err("delete must fail closed");

        assert!(error.to_string().contains("refused"));
        assert!(!database.exists());
    }

    fn create_database(path: &Path) {
        let connection = Connection::open(path).expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE project (
                   id TEXT PRIMARY KEY,
                   worktree TEXT NOT NULL,
                   name TEXT,
                   time_created INTEGER,
                   time_updated INTEGER
                 );
                 CREATE TABLE session (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
                   parent_id TEXT,
                   slug TEXT NOT NULL,
                   directory TEXT NOT NULL,
                   title TEXT NOT NULL,
                   version TEXT NOT NULL,
                   time_created INTEGER NOT NULL,
                   time_updated INTEGER NOT NULL,
                   time_archived INTEGER
                 );
                 CREATE TABLE message (
                   id TEXT PRIMARY KEY,
                   session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
                   time_created INTEGER NOT NULL,
                   data TEXT NOT NULL
                 );
                 CREATE TABLE part (
                   id TEXT PRIMARY KEY,
                   message_id TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
                   session_id TEXT NOT NULL,
                   data TEXT NOT NULL
                 );",
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO project (id, worktree, name) VALUES ('project-1', '/tmp/example', 'Example')",
                [],
            )
            .expect("project");
        for (id, parent, title, time) in [
            (ROOT_ID, None, "Root", 1_700_000_000_000_i64),
            (CHILD_ID, Some(ROOT_ID), "Child", 1_700_000_100_000_i64),
        ] {
            connection
                .execute(
                    "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated) \
                     VALUES (?1, 'project-1', ?2, ?1, '/tmp/example', ?3, '1.0.0', ?4, ?4)",
                    params![id, parent, title, time],
                )
                .expect("session");
        }
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, data) \
                 VALUES ('msg_1', ?1, 1700000000000, '{\"role\":\"user\"}')",
                [ROOT_ID],
            )
            .expect("message");
        connection
            .execute(
                "INSERT INTO part (id, message_id, session_id, data) \
                 VALUES ('part_1', 'msg_1', ?1, '{\"type\":\"text\",\"text\":\"private transcript body\"}')",
                [ROOT_ID],
            )
            .expect("part");
    }
}
