//! OpenCode storage discovery and supported CLI/Server API session operations.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration as StdDuration, SystemTime};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentCapabilities, AgentDetectionState, AgentInstallation, AgentKind,
    CategorySummary, CleanerError, CleanupItem, ContentBlock, InventorySnapshot, ItemContentDetail,
    ItemThumbnail, MemoryCapabilities, ProjectGroup, RiskLevel, SessionRecord, StorageCategory,
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
const SERVER_TIMEOUT: StdDuration = StdDuration::from_secs(2);
const SERVER_RESPONSE_LIMIT: usize = 1024 * 1024;
const SERVER_DIRECTORY_LIMIT: usize = 128;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LocalServerEndpoint {
    address: SocketAddr,
}

#[derive(Debug, Clone)]
struct WriterProcess {
    endpoint: Option<LocalServerEndpoint>,
}

#[derive(Debug, Clone, Default)]
struct RuntimeProbe {
    writers: Vec<WriterProcess>,
}

impl RuntimeProbe {
    fn running(&self) -> bool {
        !self.writers.is_empty()
    }
}

#[derive(Debug, Clone)]
struct SessionProbeRecord {
    id: String,
    directory: String,
    parent_id: Option<String>,
}

#[derive(Debug, Clone)]
enum SessionAccess {
    Offline,
    Verified {
        active_ids: HashSet<String>,
        endpoints: Vec<LocalServerEndpoint>,
    },
    Unavailable(String),
}

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
        let running = database
            .as_deref()
            .map(runtime_for_database)
            .map(|runtime| runtime.running())
            .unwrap_or_else(opencode_is_running);
        if database.is_some() && !recognized_database {
            warnings.push(
                "OpenCode database schema is not recognized; its contents remain read-only".into(),
            );
        }
        if running {
            warnings.push(
                "OpenCode is running; inactive session deletion requires a verified loopback Server API, while logs, caches, export, and restore remain blocked"
                    .into(),
            );
        }

        let thread_delete = recognized_database && binary.is_some();
        Ok(AgentInstallation {
            kind: AgentKind::OpenCode,
            state: AgentDetectionState::from_presence(binary.is_some(), recognized_database),
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
            match scan_database(&database, &mut warnings) {
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
        validate_opencode_installation(installation)?;
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
            delete_session_safely(installation, &database, session_id).await?;
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

#[derive(Debug, Clone)]
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

fn scan_database(
    database: &Path,
    warnings: &mut Vec<String>,
) -> Result<DatabaseInventory, CleanerError> {
    let runtime = runtime_for_database(database);
    scan_database_with_runtime(database, warnings, &runtime)
}

fn scan_database_with_runtime(
    database: &Path,
    warnings: &mut Vec<String>,
    runtime: &RuntimeProbe,
) -> Result<DatabaseInventory, CleanerError> {
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
    let probe_records = records
        .iter()
        .map(|session| SessionProbeRecord {
            id: session.id.clone(),
            directory: session.directory.clone(),
            parent_id: session.parent_id.clone(),
        })
        .collect::<Vec<_>>();
    let access = inspect_session_access(runtime, &probe_records);
    let access_unavailable = matches!(&access, SessionAccess::Unavailable(_));
    let (active_ids, runtime_blocker) = match &access {
        SessionAccess::Offline => (HashSet::new(), None),
        SessionAccess::Verified { active_ids, .. } => {
            warnings.push(
                "Verified OpenCode's loopback Server API; busy and retrying sessions remain blocked while inactive sessions can be deleted"
                    .into(),
            );
            (active_ids.clone(), None)
        }
        SessionAccess::Unavailable(reason) => {
            warnings.push(reason.clone());
            (HashSet::new(), Some(reason.clone()))
        }
    };
    let global_blocker = safety_blocker.or(runtime_blocker);
    let mut sessions = Vec::new();
    let mut items = Vec::new();
    for session in &records {
        let active = active_ids.contains(&session.id);
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
            status: if active {
                "active"
            } else if access_unavailable {
                "loaded"
            } else {
                "notLoaded"
            }
            .into(),
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
            blocked_reason: global_blocker.clone().or_else(|| {
                active.then(|| {
                    "OpenCode reports this session as busy or retrying; wait for it to become idle and rescan"
                        .into()
                })
            }),
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

fn validate_opencode_installation(installation: &AgentInstallation) -> Result<(), CleanerError> {
    if installation.kind != AgentKind::OpenCode {
        return Err(CleanerError::InvalidRequest(
            "OpenCode operation used a different Agent installation".into(),
        ));
    }
    Ok(())
}

fn validate_offline_installation(installation: &AgentInstallation) -> Result<(), CleanerError> {
    validate_opencode_installation(installation)?;
    let database = database_for_installation(installation)?;
    if runtime_for_database(&database).running() {
        return Err(CleanerError::Blocked(
            "Quit all related OpenCode clients before exporting or restoring session data".into(),
        ));
    }
    Ok(())
}

async fn delete_session_safely(
    installation: &AgentInstallation,
    database: &Path,
    session_id: &str,
) -> Result<(), CleanerError> {
    let runtime = runtime_for_database(database);
    delete_session_with_runtime(installation, database, session_id, &runtime).await
}

async fn delete_session_with_runtime(
    installation: &AgentInstallation,
    database: &Path,
    session_id: &str,
    runtime: &RuntimeProbe,
) -> Result<(), CleanerError> {
    let connection = open_database(database)?;
    recognized_schema(&connection)?;
    let scope = session_mutation_scope(&connection, session_id)?;
    drop(connection);

    match inspect_session_access(runtime, &scope) {
        SessionAccess::Offline => {
            let binary = installation
                .binary
                .as_deref()
                .ok_or_else(|| CleanerError::NotFound("OpenCode executable".into()))?;
            run_delete(binary, database, session_id).await
        }
        SessionAccess::Unavailable(reason) => Err(CleanerError::Blocked(reason)),
        SessionAccess::Verified {
            active_ids,
            endpoints,
        } => {
            let mut blocked = scope
                .iter()
                .filter(|session| active_ids.contains(&session.id))
                .map(|session| session.id.clone())
                .collect::<Vec<_>>();
            blocked.sort();
            if !blocked.is_empty() {
                return Err(CleanerError::Blocked(format!(
                    "OpenCode reports active session data in this deletion scope: {}",
                    blocked.join(", ")
                )));
            }
            let endpoint = endpoints.first().ok_or_else(|| {
                CleanerError::Blocked(
                    "OpenCode is running but no verified loopback Server API is available".into(),
                )
            })?;
            let directory = scope
                .iter()
                .find(|session| session.id == session_id)
                .map(|session| session.directory.as_str())
                .ok_or_else(|| CleanerError::NotFound(format!("OpenCode session {session_id}")))?;
            delete_via_server(*endpoint, session_id, directory)
        }
    }
}

fn session_mutation_scope(
    connection: &Connection,
    root: &str,
) -> Result<Vec<SessionProbeRecord>, CleanerError> {
    let mut statement = connection.prepare("SELECT id, directory, parent_id FROM session")?;
    let records = statement
        .query_map([], |row| {
            Ok(SessionProbeRecord {
                id: row.get(0)?,
                directory: row.get(1)?,
                parent_id: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let by_id = records
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect::<HashMap<_, _>>();
    if !by_id.contains_key(root) {
        return Err(CleanerError::NotFound(format!("OpenCode session {root}")));
    }
    let mut scope = Vec::new();
    let mut pending = vec![root.to_owned()];
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(session) = by_id.get(id.as_str()) {
            scope.push((*session).clone());
            pending.extend(
                records
                    .iter()
                    .filter(|candidate| candidate.parent_id.as_deref() == Some(id.as_str()))
                    .map(|candidate| candidate.id.clone()),
            );
        }
    }
    Ok(scope)
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

fn runtime_for_database(database: &Path) -> RuntimeProbe {
    let system = System::new_all();
    let writers = system
        .processes()
        .values()
        .filter(|process| is_opencode_process(process))
        .filter(|process| !is_remote_client(process.cmd()))
        .filter_map(
            |process| match process_targets_database(process.environ(), database) {
                Some(false) => None,
                Some(true) => Some(WriterProcess {
                    endpoint: explicit_loopback_endpoint(process.cmd()),
                }),
                None => Some(WriterProcess { endpoint: None }),
            },
        )
        .collect();
    RuntimeProbe { writers }
}

fn opencode_is_running() -> bool {
    let system = System::new_all();
    system.processes().values().any(is_opencode_process)
}

fn is_opencode_process(process: &sysinfo::Process) -> bool {
    let fixed_names = [
        Some(process.name().to_string_lossy().to_ascii_lowercase()),
        process
            .exe()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().to_ascii_lowercase()),
    ];
    fixed_names
        .into_iter()
        .flatten()
        .chain(
            process
                .cmd()
                .iter()
                .take(2)
                .filter_map(|value| Path::new(value).file_name())
                .map(|value| value.to_string_lossy().to_ascii_lowercase()),
        )
        .any(|name| is_opencode_process_name(&name))
}

fn is_opencode_process_name(name: &str) -> bool {
    matches!(
        name,
        "opencode" | "opencode.exe" | "opencode2" | "opencode2.exe"
    ) || name.starts_with("opencode-")
        || name.starts_with("opencode2-")
}

fn is_remote_client(arguments: &[OsString]) -> bool {
    arguments.iter().skip(1).any(|argument| {
        let argument = argument.to_string_lossy();
        argument == "attach" || argument == "--attach" || argument.starts_with("--attach=")
    })
}

fn process_targets_database(environment: &[OsString], database: &Path) -> Option<bool> {
    if environment.is_empty() {
        return None;
    }
    let data_root = process_environment_value(environment, "XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            process_environment_value(environment, "HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        })?
        .join("opencode");
    if let Some(value) = process_environment_value(environment, "OPENCODE_DB") {
        if value == ":memory:" {
            return Some(false);
        }
        let configured = PathBuf::from(value);
        let configured = if configured.is_absolute() {
            configured
        } else {
            data_root.join(configured)
        };
        return Some(paths_match(&configured, database));
    }
    Some(
        database
            .parent()
            .is_some_and(|parent| paths_match(parent, &data_root)),
    )
}

fn process_environment_value(environment: &[OsString], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    environment.iter().find_map(|entry| {
        entry
            .to_string_lossy()
            .strip_prefix(&prefix)
            .map(str::to_owned)
    })
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn explicit_loopback_endpoint(arguments: &[OsString]) -> Option<LocalServerEndpoint> {
    let port = argument_value(arguments, "--port")?.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    let hostname = argument_value(arguments, "--hostname")
        .unwrap_or_else(|| "127.0.0.1".into())
        .to_ascii_lowercase();
    let ip = match hostname.as_str() {
        "localhost" | "127.0.0.1" | "0.0.0.0" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        "::1" | "[::1]" | "::" | "[::]" => IpAddr::V6(Ipv6Addr::LOCALHOST),
        _ => hostname
            .parse::<IpAddr>()
            .ok()
            .filter(IpAddr::is_loopback)?,
    };
    Some(LocalServerEndpoint {
        address: SocketAddr::new(ip, port),
    })
}

fn argument_value(arguments: &[OsString], name: &str) -> Option<String> {
    for (index, argument) in arguments.iter().enumerate().skip(1) {
        let argument = argument.to_string_lossy();
        if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
            return Some(value.to_owned());
        }
        if argument == name {
            return arguments
                .get(index + 1)
                .map(|value| value.to_string_lossy().into_owned());
        }
    }
    None
}

fn inspect_session_access(runtime: &RuntimeProbe, records: &[SessionProbeRecord]) -> SessionAccess {
    if !runtime.running() {
        return SessionAccess::Offline;
    }
    if runtime
        .writers
        .iter()
        .any(|writer| writer.endpoint.is_none())
    {
        return SessionAccess::Unavailable(unverified_server_message());
    }
    let endpoints = runtime
        .writers
        .iter()
        .filter_map(|writer| writer.endpoint)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if endpoints.is_empty() {
        return SessionAccess::Unavailable(unverified_server_message());
    }

    let directories = records
        .iter()
        .map(|session| session.directory.clone())
        .collect::<BTreeSet<_>>();
    if directories.len() > SERVER_DIRECTORY_LIMIT {
        return SessionAccess::Unavailable(format!(
            "OpenCode is running across more than {SERVER_DIRECTORY_LIMIT} session directories; quit it before deleting sessions"
        ));
    }
    let sample = records.first();
    let mut active_ids = HashSet::new();
    for endpoint in &endpoints {
        let health = match server_json::<ServerHealth>(*endpoint, "GET", "/global/health") {
            Ok(health) if health.healthy => health,
            _ => return SessionAccess::Unavailable(unverified_server_message()),
        };
        let _ = health;
        if let Some(sample) = sample {
            let path = session_server_path(&sample.id, &sample.directory);
            let found = match server_json::<ServerSession>(*endpoint, "GET", &path) {
                Ok(found) => found,
                Err(_) => return SessionAccess::Unavailable(unverified_server_message()),
            };
            if found.id != sample.id
                || !paths_match(Path::new(&found.directory), Path::new(&sample.directory))
            {
                return SessionAccess::Unavailable(unverified_server_message());
            }
        }
        for directory in &directories {
            let path = format!(
                "/session/status?directory={}",
                percent_encode_query(directory)
            );
            let statuses = match server_json::<HashMap<String, ServerSessionStatus>>(
                *endpoint, "GET", &path,
            ) {
                Ok(statuses) => statuses,
                Err(_) => return SessionAccess::Unavailable(unverified_server_message()),
            };
            active_ids.extend(statuses.into_iter().filter_map(|(id, status)| {
                matches!(status.kind.as_str(), "busy" | "retry").then_some(id)
            }));
        }
    }
    SessionAccess::Verified {
        active_ids,
        endpoints,
    }
}

fn unverified_server_message() -> String {
    "OpenCode is running without a verified loopback Server API; quit it or relaunch it with an explicit loopback --port before deleting sessions".into()
}

#[derive(Debug, Deserialize)]
struct ServerHealth {
    healthy: bool,
}

#[derive(Debug, Deserialize)]
struct ServerSession {
    id: String,
    directory: String,
}

#[derive(Debug, Deserialize)]
struct ServerSessionStatus {
    #[serde(rename = "type")]
    kind: String,
}

fn session_server_path(session_id: &str, directory: &str) -> String {
    format!(
        "/session/{session_id}?directory={}",
        percent_encode_query(directory)
    )
}

fn delete_via_server(
    endpoint: LocalServerEndpoint,
    session_id: &str,
    directory: &str,
) -> Result<(), CleanerError> {
    let deleted = server_json::<bool>(
        endpoint,
        "DELETE",
        &session_server_path(session_id, directory),
    )?;
    if !deleted {
        return Err(CleanerError::Integration(format!(
            "OpenCode Server API did not delete session {session_id}"
        )));
    }
    Ok(())
}

fn server_json<T: serde::de::DeserializeOwned>(
    endpoint: LocalServerEndpoint,
    method: &str,
    path: &str,
) -> Result<T, CleanerError> {
    let body = server_request(endpoint, method, path)?;
    serde_json::from_slice(&body).map_err(CleanerError::Json)
}

fn server_request(
    endpoint: LocalServerEndpoint,
    method: &str,
    path: &str,
) -> Result<Vec<u8>, CleanerError> {
    if !endpoint.address.ip().is_loopback() || !path.starts_with('/') {
        return Err(CleanerError::UnsafePath(format!(
            "OpenCode Server API endpoint {}",
            endpoint.address
        )));
    }
    let mut stream = TcpStream::connect_timeout(&endpoint.address, SERVER_TIMEOUT)?;
    stream.set_read_timeout(Some(SERVER_TIMEOUT))?;
    stream.set_write_timeout(Some(SERVER_TIMEOUT))?;
    let host = match endpoint.address.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{}\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        endpoint.address.port()
    )?;
    stream.flush()?;
    let mut response = Vec::new();
    loop {
        let mut buffer = [0_u8; 8192];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                response.extend_from_slice(&buffer[..length]);
                if response.len() > SERVER_RESPONSE_LIMIT {
                    return Err(CleanerError::Integration(
                        "OpenCode Server API response exceeded the safety limit".into(),
                    ));
                }
                if http_response_complete(&response) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && http_response_complete(&response) =>
            {
                break;
            }
            Err(error) => return Err(CleanerError::Io(error)),
        }
    }
    parse_http_response(&response)
}

fn http_response_complete(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let body = &response[header_end + 4..];
    for line in headers.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .is_ok_and(|length| body.len() >= length);
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            return decode_chunked_body(body).is_ok();
        }
    }
    false
}

fn parse_http_response(response: &[u8]) -> Result<Vec<u8>, CleanerError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| CleanerError::Integration("Invalid OpenCode Server API response".into()))?;
    let headers = std::str::from_utf8(&response[..header_end]).map_err(|_| {
        CleanerError::Integration("Invalid OpenCode Server API response headers".into())
    })?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| CleanerError::Integration("Invalid OpenCode Server API status".into()))?;
    if status != 200 {
        return Err(CleanerError::Integration(format!(
            "OpenCode Server API returned HTTP {status}"
        )));
    }
    let body = &response[header_end + 4..];
    let chunked = headers.lines().skip(1).any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });
    if chunked {
        decode_chunked_body(body)
    } else {
        Ok(body.to_vec())
    }
}

fn decode_chunked_body(mut input: &[u8]) -> Result<Vec<u8>, CleanerError> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| CleanerError::Integration("Invalid chunked response".into()))?;
        let size = std::str::from_utf8(&input[..line_end])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|value| usize::from_str_radix(value.trim(), 16).ok())
            .ok_or_else(|| CleanerError::Integration("Invalid chunked response".into()))?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err(CleanerError::Integration(
                "Truncated chunked response".into(),
            ));
        }
        output.extend_from_slice(&input[..size]);
        if output.len() > SERVER_RESPONSE_LIMIT {
            return Err(CleanerError::Integration(
                "OpenCode Server API response exceeded the safety limit".into(),
            ));
        }
        input = &input[size + 2..];
    }
}

fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

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
        // The mock is deliberately not named `opencode` so a concurrently running process-probe
        // test never mistakes this short-lived CLI mock for a live OpenCode writer.
        let binary = fixture.path().join("mock-opencode");
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
        let binary = fixture.path().join("mock-opencode");
        let database = fixture.path().join("opencode.db");
        fs::write(&binary, "#!/bin/sh\necho 'refused' >&2\nexit 23\n").expect("mock binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");

        let error = run_delete(binary.to_str().expect("binary path"), &database, ROOT_ID)
            .await
            .expect_err("delete must fail closed");

        assert!(error.to_string().contains("refused"));
        assert!(!database.exists());
    }

    #[test]
    fn only_explicit_loopback_ports_are_discoverable() {
        let loopback = [
            OsString::from("opencode"),
            OsString::from("serve"),
            OsString::from("--port=4096"),
        ];
        assert_eq!(
            explicit_loopback_endpoint(&loopback),
            Some(LocalServerEndpoint {
                address: "127.0.0.1:4096".parse().expect("socket address"),
            })
        );

        let dynamic = [
            OsString::from("opencode"),
            OsString::from("--port"),
            OsString::from("0"),
        ];
        assert_eq!(explicit_loopback_endpoint(&dynamic), None);

        let remote = [
            OsString::from("opencode"),
            OsString::from("serve"),
            OsString::from("--port"),
            OsString::from("4096"),
            OsString::from("--hostname"),
            OsString::from("192.0.2.10"),
        ];
        assert_eq!(explicit_loopback_endpoint(&remote), None);
    }

    #[cfg(unix)]
    #[test]
    fn native_process_probe_matches_an_explicit_database() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().expect("process fixture");
        let binary = fixture.path().join("opencode");
        let database = fixture.path().join("opencode.db");
        fs::write(&binary, "#!/bin/sh\nsleep 5\n").expect("mock executable");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");
        fs::write(&database, b"database identity").expect("database path");
        let mut child = std::process::Command::new(&binary)
            .arg("serve")
            .arg("--port=41837")
            .env("OPENCODE_DB", &database)
            .spawn()
            .expect("mock OpenCode process");

        let runtime = (0..20)
            .find_map(|_| {
                let runtime = runtime_for_database(&database);
                if runtime.running() {
                    Some(runtime)
                } else {
                    std::thread::sleep(StdDuration::from_millis(25));
                    None
                }
            })
            .expect("running OpenCode process");
        child.kill().expect("stop mock process");
        child.wait().expect("reap mock process");

        assert_eq!(runtime.writers.len(), 1);
    }

    #[test]
    #[ignore = "requires a local OpenCode installation"]
    fn live_verifies_an_isolated_loopback_server() {
        let binary = find_opencode_binary().expect("OpenCode executable");
        let fixture = tempfile::tempdir().expect("isolated OpenCode home");
        let database = fixture.path().join("opencode.db");
        let project = fixture.path().join("project");
        fs::create_dir_all(&project).expect("isolated project directory");
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("available port");
            listener.local_addr().expect("listener address").port()
        };
        let mut child = std::process::Command::new(binary)
            .arg("--pure")
            .arg("serve")
            .arg(format!("--port={port}"))
            .env("OPENCODE_DB", &database)
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("isolated OpenCode server");

        let verified = (0..100).find_map(|_| {
            let runtime = runtime_for_database(&database);
            let endpoint = runtime.writers.iter().find_map(|writer| writer.endpoint)?;
            match server_json::<ServerHealth>(endpoint, "GET", "/global/health") {
                Ok(health) if health.healthy => Some(endpoint),
                _ => {
                    std::thread::sleep(StdDuration::from_millis(50));
                    None
                }
            }
        });
        let endpoint = verified.expect("verified loopback server");
        assert_eq!(
            endpoint,
            LocalServerEndpoint {
                address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            }
        );
        let directory = project.to_string_lossy().into_owned();
        let created = server_json::<ServerSession>(
            endpoint,
            "POST",
            &format!("/session?directory={}", percent_encode_query(&directory)),
        )
        .expect("create isolated session");
        assert!(paths_match(
            Path::new(&created.directory),
            Path::new(&directory)
        ));
        let runtime = runtime_for_database(&database);
        assert!(matches!(
            inspect_session_access(
                &runtime,
                &[SessionProbeRecord {
                    id: created.id.clone(),
                    directory: created.directory.clone(),
                    parent_id: None,
                }]
            ),
            SessionAccess::Verified { ref active_ids, .. } if active_ids.is_empty()
        ));
        delete_via_server(endpoint, &created.id, &created.directory)
            .expect("delete isolated session");
        let connection = open_database(&database).expect("isolated database");
        recognized_schema(&connection).expect("recognized isolated schema");
        assert!(!session_exists(&connection, &created.id).expect("deleted session state"));
        drop(connection);

        child.kill().expect("stop isolated OpenCode server");
        child.wait().expect("reap isolated OpenCode server");
    }

    #[test]
    fn an_unverifiable_writer_keeps_every_session_blocked() {
        let access = inspect_session_access(
            &RuntimeProbe {
                writers: vec![WriterProcess { endpoint: None }],
            },
            &probe_records(),
        );

        assert!(matches!(access, SessionAccess::Unavailable(_)));
    }

    #[test]
    fn verified_server_status_blocks_only_busy_sessions() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let database = fixture.path().join("opencode.db");
        create_database(&database);
        let (endpoint, requests, handle) =
            mock_server(3, format!(r#"{{"{CHILD_ID}":{{"type":"busy"}}}}"#));
        let (_, _, items) = scan_database_with_runtime(
            &database,
            &mut Vec::new(),
            &RuntimeProbe {
                writers: vec![WriterProcess {
                    endpoint: Some(endpoint),
                }],
            },
        )
        .expect("running inventory");
        handle.join().expect("mock server");

        assert!(
            items
                .iter()
                .find(|item| item.thread_id.as_deref() == Some(ROOT_ID))
                .expect("idle root")
                .blocked_reason
                .is_none()
        );
        assert!(
            items
                .iter()
                .find(|item| item.thread_id.as_deref() == Some(CHILD_ID))
                .expect("busy child")
                .blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("busy or retrying"))
        );
        assert_eq!(requests.lock().expect("requests").len(), 3);
    }

    #[tokio::test]
    async fn running_deletion_uses_verified_server_api_for_an_idle_scope() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let database = fixture.path().join("opencode.db");
        create_database(&database);
        let (endpoint, requests, handle) = mock_server(4, "{}".into());
        let installation = fixture_installation(fixture.path());

        delete_session_with_runtime(
            &installation,
            &database,
            ROOT_ID,
            &RuntimeProbe {
                writers: vec![WriterProcess {
                    endpoint: Some(endpoint),
                }],
            },
        )
        .await
        .expect("server deletion");
        handle.join().expect("mock server");

        let requests = requests.lock().expect("requests");
        assert!(
            requests
                .iter()
                .any(|request| request.starts_with(&format!("DELETE /session/{ROOT_ID}?")))
        );
    }

    #[tokio::test]
    async fn active_descendant_blocks_server_deletion_before_mutation() {
        let fixture = tempfile::tempdir().expect("OpenCode fixture");
        let database = fixture.path().join("opencode.db");
        create_database(&database);
        let (endpoint, requests, handle) =
            mock_server(3, format!(r#"{{"{CHILD_ID}":{{"type":"retry"}}}}"#));
        let installation = fixture_installation(fixture.path());

        let error = delete_session_with_runtime(
            &installation,
            &database,
            ROOT_ID,
            &RuntimeProbe {
                writers: vec![WriterProcess {
                    endpoint: Some(endpoint),
                }],
            },
        )
        .await
        .expect_err("active descendant must block deletion");
        handle.join().expect("mock server");

        assert!(error.to_string().contains(CHILD_ID));
        assert!(
            requests
                .lock()
                .expect("requests")
                .iter()
                .all(|request| !request.starts_with("DELETE "))
        );
        assert!(
            session_exists(&open_database(&database).expect("database"), ROOT_ID)
                .expect("root still exists")
        );
    }

    fn probe_records() -> Vec<SessionProbeRecord> {
        vec![
            SessionProbeRecord {
                id: ROOT_ID.into(),
                directory: "/tmp/example".into(),
                parent_id: None,
            },
            SessionProbeRecord {
                id: CHILD_ID.into(),
                directory: "/tmp/example".into(),
                parent_id: Some(ROOT_ID.into()),
            },
        ]
    }

    fn fixture_installation(home: &Path) -> AgentInstallation {
        AgentInstallation {
            kind: AgentKind::OpenCode,
            state: Default::default(),
            home: home.to_string_lossy().into_owned(),
            binary: None,
            version: Some("test".into()),
            app_support: None,
            running: true,
            capabilities: AgentCapabilities {
                thread_list: true,
                thread_delete: true,
                memory: MemoryCapabilities::default(),
                descendant_filter: true,
                report_only: false,
            },
            warnings: Vec::new(),
        }
    }

    type MockRequests = Arc<Mutex<Vec<String>>>;

    fn mock_server(
        expected_requests: usize,
        statuses: String,
    ) -> (LocalServerEndpoint, MockRequests, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock server listener");
        let endpoint = LocalServerEndpoint {
            address: listener.local_addr().expect("listener address"),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let handle = thread::spawn(move || {
            for _ in 0..expected_requests {
                let (mut stream, _) = listener.accept().expect("mock request");
                stream
                    .set_read_timeout(Some(StdDuration::from_secs(2)))
                    .expect("read timeout");
                let mut request_bytes = Vec::new();
                while !request_bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut buffer = [0_u8; 1024];
                    let length = stream.read(&mut buffer).expect("read request");
                    if length == 0 {
                        break;
                    }
                    request_bytes.extend_from_slice(&buffer[..length]);
                }
                let request = String::from_utf8_lossy(&request_bytes);
                let request_line = request.lines().next().unwrap_or_default().to_owned();
                captured
                    .lock()
                    .expect("captured requests")
                    .push(request_line.clone());
                let body = if request_line.starts_with("GET /global/health ") {
                    r#"{"healthy":true,"version":"test"}"#.to_owned()
                } else if request_line.starts_with("GET /session/status?") {
                    statuses.clone()
                } else if request_line.starts_with("GET /session/") {
                    let id = request_line
                        .strip_prefix("GET /session/")
                        .and_then(|value| value.split('?').next())
                        .unwrap_or(ROOT_ID);
                    format!(r#"{{"id":"{id}","directory":"/tmp/example"}}"#)
                } else if request_line.starts_with("DELETE /session/") {
                    "true".to_owned()
                } else {
                    "null".to_owned()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write response");
                stream.flush().expect("flush response");
            }
        });
        (endpoint, requests, handle)
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
