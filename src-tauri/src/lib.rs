use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use adapter_claude::ClaudeCodeAdapter;
use adapter_codex::CodexAdapter;
use chrono::{DateTime, Duration, Utc};
use cleanerx_core::{
    AgentAdapter, AgentInstallation, AgentKind, AppSettings, BackupRecord, BackupSource,
    BackupStore, CategorySummary, CleanerError, CleanupItem, CleanupPlan, CleanupResult,
    FileIdentity, InventorySnapshot, ItemContentDetail, ItemThumbnail, OperationKind,
    OperationStatus, PathPolicy, SessionRecord, StorageCategory, create_cleanup_plan, safe_remove,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::menu::Menu;
use tauri::{Manager, State};
use uuid::Uuid;

struct AppState {
    codex_adapter: CodexAdapter,
    claude_adapter: ClaudeCodeAdapter,
    data_dir: PathBuf,
    settings: Mutex<AppSettings>,
    snapshot: Mutex<Option<InventorySnapshot>>,
    plans: Mutex<HashMap<Uuid, CleanupPlan>>,
}

impl AppState {
    fn adapter(&self, kind: AgentKind) -> &dyn AgentAdapter {
        match kind {
            AgentKind::Codex => &self.codex_adapter,
            AgentKind::ClaudeCode => &self.claude_adapter,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationJournal {
    operation_id: Uuid,
    status: OperationStatus,
    updated_at: DateTime<Utc>,
    backup_id: Option<Uuid>,
    message: Option<String>,
}

const UNASSIGNED_PROJECT_ID: &str = "__no_project";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionProjectSummary {
    id: String,
    name: String,
    roots: Vec<String>,
    session_count: usize,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionProjectResult {
    projects: Vec<SessionProjectSummary>,
    unassigned_session_count: usize,
    unassigned_session_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InventoryReport {
    id: Uuid,
    scanned_at: DateTime<Utc>,
    installation: AgentInstallation,
    total_bytes: u64,
    reclaimable_bytes: u64,
    items: Vec<CleanupItem>,
    projects: Vec<SessionProjectSummary>,
    categories: Vec<CategorySummary>,
    warnings: Vec<String>,
    session_count: usize,
    archived_session_count: usize,
    session_sources: Vec<String>,
    unassigned_session_count: usize,
    unassigned_session_size_bytes: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionFilter {
    snapshot_id: Uuid,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    query: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    updated_within_days: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionPageRequest {
    #[serde(flatten)]
    filter: SessionFilter,
    cursor: usize,
    limit: usize,
    include_ancestors: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionPage {
    snapshot_id: Uuid,
    sessions: Vec<SessionRecord>,
    items: Vec<CleanupItem>,
    matching_session_ids: Vec<String>,
    total_count: usize,
    next_cursor: Option<usize>,
}

type CommandResult<T> = Result<T, String>;

#[tauri::command]
async fn detect_agents(state: State<'_, AppState>) -> CommandResult<Vec<AgentInstallation>> {
    let settings = state.settings.lock().clone();
    let (codex, claude) = tokio::join!(
        state
            .codex_adapter
            .detect(settings.custom_codex_home.as_deref()),
        state
            .claude_adapter
            .detect(settings.custom_claude_home.as_deref())
    );
    Ok(vec![
        codex.map_err(error_message)?,
        claude.map_err(error_message)?,
    ])
}

#[tauri::command]
async fn scan_storage(
    target_agent: Option<AgentKind>,
    state: State<'_, AppState>,
) -> CommandResult<InventoryReport> {
    let settings = state.settings.lock().clone();
    let kind = target_agent.unwrap_or(settings.active_agent);
    let snapshot = state
        .adapter(kind)
        .scan(custom_home(&settings, kind))
        .await
        .map_err(error_message)?;
    let report = inventory_report(&snapshot);
    *state.snapshot.lock() = Some(snapshot);
    Ok(report)
}

#[tauri::command]
fn get_session_projects(
    filter: SessionFilter,
    state: State<'_, AppState>,
) -> CommandResult<SessionProjectResult> {
    with_current_snapshot(&state, filter.snapshot_id, |snapshot| {
        session_project_result(snapshot, &filter)
    })
}

#[tauri::command]
fn get_session_page(
    request: SessionPageRequest,
    state: State<'_, AppState>,
) -> CommandResult<SessionPage> {
    with_current_snapshot(&state, request.filter.snapshot_id, |snapshot| {
        session_page(snapshot, &request)
    })
}

fn with_current_snapshot<T>(
    state: &AppState,
    snapshot_id: Uuid,
    read: impl FnOnce(&InventorySnapshot) -> CommandResult<T>,
) -> CommandResult<T> {
    let snapshot_guard = state.snapshot.lock();
    let snapshot = snapshot_guard
        .as_ref()
        .ok_or_else(|| "Scan storage before loading sessions".to_owned())?;
    if snapshot.id != snapshot_id {
        return Err("The requested session page does not match the current scan".into());
    }
    read(snapshot)
}

fn inventory_report(snapshot: &InventorySnapshot) -> InventoryReport {
    let filter = SessionFilter {
        snapshot_id: snapshot.id,
        ..SessionFilter::default()
    };
    let project_result = session_project_result(snapshot, &filter)
        .expect("an unfiltered inventory snapshot always has a valid session query");
    let session_sources = snapshot
        .sessions
        .iter()
        .map(|session| session.source.clone())
        .collect::<BTreeSet<_>>();
    InventoryReport {
        id: snapshot.id,
        scanned_at: snapshot.scanned_at,
        installation: snapshot.installation.clone(),
        total_bytes: snapshot.total_bytes,
        reclaimable_bytes: snapshot.reclaimable_bytes,
        items: snapshot
            .items
            .iter()
            .filter(|item| item.thread_id.is_none())
            .cloned()
            .collect(),
        projects: project_result.projects,
        categories: snapshot.categories.clone(),
        warnings: snapshot.warnings.clone(),
        session_count: snapshot.sessions.len(),
        archived_session_count: snapshot
            .sessions
            .iter()
            .filter(|session| session.archived)
            .count(),
        session_sources: session_sources.into_iter().collect(),
        unassigned_session_count: project_result.unassigned_session_count,
        unassigned_session_size_bytes: project_result.unassigned_session_size_bytes,
    }
}

fn session_project_result(
    snapshot: &InventorySnapshot,
    filter: &SessionFilter,
) -> CommandResult<SessionProjectResult> {
    let matching = matching_sessions(snapshot, filter)?;
    let item_by_thread: HashMap<&str, &CleanupItem> = snapshot
        .items
        .iter()
        .filter_map(|item| item.thread_id.as_deref().map(|id| (id, item)))
        .collect();
    let mut counts: HashMap<&str, (usize, u64)> = HashMap::new();
    let mut unassigned_session_count = 0;
    let mut unassigned_session_size_bytes = 0u64;
    for session in matching {
        match item_by_thread
            .get(session.id.as_str())
            .and_then(|item| item.project_id.as_deref())
        {
            Some(project_id) => {
                let summary = counts.entry(project_id).or_default();
                summary.0 += 1;
                summary.1 = summary.1.saturating_add(session.size_bytes);
            }
            None => {
                unassigned_session_count += 1;
                unassigned_session_size_bytes =
                    unassigned_session_size_bytes.saturating_add(session.size_bytes);
            }
        }
    }
    let projects = snapshot
        .projects
        .iter()
        .filter_map(|project| {
            let (session_count, size_bytes) = counts.get(project.id.as_str()).copied()?;
            Some(SessionProjectSummary {
                id: project.id.clone(),
                name: project.name.clone(),
                roots: project.roots.clone(),
                session_count,
                size_bytes,
            })
        })
        .collect();
    Ok(SessionProjectResult {
        projects,
        unassigned_session_count,
        unassigned_session_size_bytes,
    })
}

fn session_page(
    snapshot: &InventorySnapshot,
    request: &SessionPageRequest,
) -> CommandResult<SessionPage> {
    if request.limit == 0 || request.limit > 100 {
        return Err("Session page size must be between 1 and 100".into());
    }
    let matching = matching_sessions(snapshot, &request.filter)?;
    let total_count = matching.len();
    if request.cursor > total_count {
        return Err("Session page cursor is outside the filtered result".into());
    }
    let end = request
        .cursor
        .saturating_add(request.limit)
        .min(total_count);
    let page = &matching[request.cursor..end];
    let matching_session_ids: Vec<String> = page.iter().map(|session| session.id.clone()).collect();
    let mut included: HashSet<&str> = page.iter().map(|session| session.id.as_str()).collect();
    if request.include_ancestors {
        let session_by_id: HashMap<&str, &SessionRecord> = snapshot
            .sessions
            .iter()
            .map(|session| (session.id.as_str(), session))
            .collect();
        for session in page {
            let mut parent_id = session.parent_thread_id.as_deref();
            let mut visited = HashSet::new();
            while let Some(id) = parent_id {
                if !visited.insert(id) {
                    break;
                }
                let Some(parent) = session_by_id.get(id) else {
                    break;
                };
                included.insert(parent.id.as_str());
                parent_id = parent.parent_thread_id.as_deref();
            }
        }
    }
    let sessions: Vec<SessionRecord> = snapshot
        .sessions
        .iter()
        .filter(|session| included.contains(session.id.as_str()))
        .cloned()
        .collect();
    let items = snapshot
        .items
        .iter()
        .filter(|item| {
            item.thread_id
                .as_deref()
                .is_some_and(|id| included.contains(id))
        })
        .cloned()
        .collect();
    Ok(SessionPage {
        snapshot_id: snapshot.id,
        sessions,
        items,
        matching_session_ids,
        total_count,
        next_cursor: (end < total_count).then_some(end),
    })
}

fn matching_sessions<'a>(
    snapshot: &'a InventorySnapshot,
    filter: &SessionFilter,
) -> CommandResult<Vec<&'a SessionRecord>> {
    validate_session_filter(snapshot, filter)?;
    let item_by_thread: HashMap<&str, &CleanupItem> = snapshot
        .items
        .iter()
        .filter_map(|item| item.thread_id.as_deref().map(|id| (id, item)))
        .collect();
    let query = filter.query.trim().to_lowercase();
    let cutoff = filter
        .updated_within_days
        .map(|days| Utc::now() - Duration::days(i64::from(days)));
    let mut sessions: Vec<_> = snapshot
        .sessions
        .iter()
        .filter(|session| {
            let item = item_by_thread.get(session.id.as_str());
            let matches_project = match filter.project_id.as_deref() {
                None => true,
                Some(UNASSIGNED_PROJECT_ID) => item.is_some_and(|item| item.project_id.is_none()),
                Some(project_id) => item
                    .and_then(|item| item.project_id.as_deref())
                    .is_some_and(|id| id == project_id),
            };
            let matches_query = query.is_empty()
                || session.name.to_lowercase().contains(&query)
                || session.cwd.to_lowercase().contains(&query);
            let matches_source = filter
                .source
                .as_deref()
                .is_none_or(|source| session.source == source);
            let matches_state = match filter.state.as_deref() {
                None => true,
                Some("archived") => session.archived,
                Some("active") => !session.archived,
                Some(_) => false,
            };
            let matches_updated = cutoff.as_ref().is_none_or(|cutoff| {
                session
                    .updated_at
                    .as_ref()
                    .is_some_and(|updated_at| updated_at >= cutoff)
            });
            matches_project && matches_query && matches_source && matches_state && matches_updated
        })
        .collect();
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(sessions)
}

fn validate_session_filter(
    snapshot: &InventorySnapshot,
    filter: &SessionFilter,
) -> CommandResult<()> {
    if snapshot.id != filter.snapshot_id {
        return Err("The session filter does not match the current scan".into());
    }
    if filter.query.len() > 512 {
        return Err("Session search is too long".into());
    }
    if filter
        .source
        .as_ref()
        .is_some_and(|source| source.len() > 128)
    {
        return Err("Session source filter is too long".into());
    }
    if filter
        .updated_within_days
        .is_some_and(|days| days == 0 || days > 3650)
    {
        return Err("Session updated-time filter is outside the supported range".into());
    }
    if filter
        .state
        .as_deref()
        .is_some_and(|state| state != "active" && state != "archived")
    {
        return Err("Session state filter is not recognized".into());
    }
    if let Some(project_id) = filter.project_id.as_deref()
        && project_id != UNASSIGNED_PROJECT_ID
        && !snapshot
            .projects
            .iter()
            .any(|project| project.id == project_id)
    {
        return Err("Session project filter is not part of the current scan".into());
    }
    Ok(())
}

#[tauri::command]
async fn get_item_content(
    item_id: String,
    state: State<'_, AppState>,
) -> CommandResult<ItemContentDetail> {
    let snapshot = state
        .snapshot
        .lock()
        .clone()
        .ok_or_else(|| "Scan storage before opening item content".to_owned())?;
    let item = snapshot
        .items
        .iter()
        .find(|item| item.id == item_id)
        .cloned()
        .ok_or_else(|| format!("Item {item_id} is not part of the current scan"))?;
    state
        .adapter(snapshot.installation.kind)
        .load_item_content(&snapshot.installation, &item)
        .await
        .map_err(error_message)
}

#[tauri::command]
async fn get_item_thumbnail(
    item_id: String,
    state: State<'_, AppState>,
) -> CommandResult<Option<ItemThumbnail>> {
    let snapshot = state
        .snapshot
        .lock()
        .clone()
        .ok_or_else(|| "Scan storage before opening an item thumbnail".to_owned())?;
    let item = snapshot
        .items
        .iter()
        .find(|item| item.id == item_id)
        .cloned()
        .ok_or_else(|| format!("Item {item_id} is not part of the current scan"))?;
    state
        .adapter(snapshot.installation.kind)
        .load_item_thumbnail(&snapshot.installation, &item)
        .await
        .map_err(error_message)
}

#[tauri::command]
fn plan_cleanup(
    selected_item_ids: Vec<String>,
    state: State<'_, AppState>,
) -> CommandResult<CleanupPlan> {
    let snapshot = state
        .snapshot
        .lock()
        .clone()
        .ok_or_else(|| "Scan storage before creating a cleanup plan".to_owned())?;
    let plan = create_cleanup_plan(&snapshot, &selected_item_ids).map_err(error_message)?;
    state.plans.lock().insert(plan.id, plan.clone());
    Ok(plan)
}

#[tauri::command]
async fn execute_cleanup(
    plan_id: Uuid,
    create_backup: bool,
    state: State<'_, AppState>,
) -> CommandResult<CleanupResult> {
    let plan = state
        .plans
        .lock()
        .get(&plan_id)
        .cloned()
        .ok_or_else(|| format!("Cleanup plan {plan_id} is no longer available"))?;
    let snapshot = state
        .snapshot
        .lock()
        .clone()
        .ok_or_else(|| "The inventory snapshot is no longer available".to_owned())?;
    if snapshot.id != plan.snapshot_id {
        return Err("The cleanup plan does not match the current scan".into());
    }
    if !plan.blockers.is_empty() {
        return Err(format!("Cleanup is blocked: {}", plan.blockers.join("; ")));
    }
    write_journal(
        &state.data_dir,
        OperationJournal {
            operation_id: plan.id,
            status: OperationStatus::Planned,
            updated_at: Utc::now(),
            backup_id: None,
            message: None,
        },
    )
    .map_err(error_message)?;

    let result = execute_plan(&state, &plan, &snapshot, create_backup).await;
    if let Err(error) = &result {
        let _ = write_journal(
            &state.data_dir,
            OperationJournal {
                operation_id: plan.id,
                status: OperationStatus::Failed,
                updated_at: Utc::now(),
                backup_id: None,
                message: Some(error.to_string()),
            },
        );
    }
    result.map_err(error_message)
}

async fn execute_plan(
    state: &AppState,
    plan: &CleanupPlan,
    snapshot: &InventorySnapshot,
    create_backup: bool,
) -> Result<CleanupResult, CleanerError> {
    let settings = state.settings.lock().clone();
    let kind = snapshot.installation.kind;
    let adapter = state.adapter(kind);
    let current_installation = adapter.detect(custom_home(&settings, kind)).await?;
    if current_installation.running
        && plan
            .operations
            .iter()
            .any(|operation| operation.requires_agent_exit)
    {
        return Err(CleanerError::Blocked(format!(
            "Quit {} and retry this cleanup plan",
            kind.display_name()
        )));
    }

    let selected_items: Vec<_> = snapshot
        .items
        .iter()
        .filter(|item| {
            plan.selected_item_ids.contains(&item.id)
                || item
                    .thread_id
                    .as_ref()
                    .is_some_and(|id| plan.expanded_session_ids.contains(id))
        })
        .collect();
    let policy = PathPolicy::new(
        allowed_roots(&snapshot.installation),
        protected_paths(&snapshot.installation),
    );
    let identities = capture_mutation_identities(&selected_items, &policy, kind)?;

    let backup_id = if !create_backup {
        None
    } else {
        let store = BackupStore::new(
            state.data_dir.join("backups"),
            settings.backup_retention_days,
        )?;
        let sources = backup_sources(&selected_items, &snapshot.installation)?;
        if sources.is_empty() {
            None
        } else {
            let manifest =
                store.create_backup(plan, kind, snapshot.installation.version.clone(), &sources)?;
            write_journal(
                &state.data_dir,
                OperationJournal {
                    operation_id: plan.id,
                    status: OperationStatus::BackupWritten,
                    updated_at: Utc::now(),
                    backup_id: Some(manifest.id),
                    message: None,
                },
            )?;
            Some(manifest.id)
        }
    };

    write_journal(
        &state.data_dir,
        OperationJournal {
            operation_id: plan.id,
            status: OperationStatus::Deleting,
            updated_at: Utc::now(),
            backup_id,
            message: None,
        },
    )?;

    let mut warnings = Vec::new();
    let mut deleted_item_ids = Vec::new();
    let before = snapshot.total_bytes;
    for operation in &plan.operations {
        match operation.kind {
            OperationKind::DeleteSession => {
                if kind == AgentKind::Codex {
                    adapter
                        .delete_sessions(&snapshot.installation, &operation.session_ids)
                        .await?;
                    cleanup_session_artifacts(
                        &selected_items,
                        &snapshot.sessions,
                        &policy,
                        &identities,
                        &mut warnings,
                    )?;
                } else {
                    remove_operation_paths(
                        operation,
                        &selected_items,
                        &policy,
                        &identities,
                        &mut warnings,
                    )?;
                }
                deleted_item_ids.extend(operation.item_ids.iter().cloned());
            }
            OperationKind::ResetMemory => {
                if current_installation.capabilities.memory.can_reset_all {
                    adapter.reset_memory(&current_installation).await?;
                } else if current_installation.capabilities.memory.can_reset_scope
                    || current_installation.capabilities.memory.can_delete_entries
                {
                    remove_operation_paths(
                        operation,
                        &selected_items,
                        &policy,
                        &identities,
                        &mut warnings,
                    )?;
                } else {
                    return Err(CleanerError::Unsupported(format!(
                        "{} memory deletion is unavailable",
                        kind.display_name()
                    )));
                }
                deleted_item_ids.extend(operation.item_ids.iter().cloned());
            }
            OperationKind::CleanRegenerable => {
                if kind == AgentKind::ClaudeCode {
                    remove_operation_paths(
                        operation,
                        &selected_items,
                        &policy,
                        &identities,
                        &mut warnings,
                    )?;
                    deleted_item_ids.extend(operation.item_ids.iter().cloned());
                } else {
                    for item_id in &operation.item_ids {
                        let Some(item) = selected_items.iter().find(|item| &item.id == item_id)
                        else {
                            continue;
                        };
                        match item.category {
                            StorageCategory::Log => clean_logs(item, settings.log_retention_days)?,
                            StorageCategory::Attachment
                            | StorageCategory::GeneratedImage
                            | StorageCategory::Cache
                            | StorageCategory::Temporary => {
                                for path in &item.paths {
                                    remove_if_unchanged(
                                        Path::new(path),
                                        &policy,
                                        &identities,
                                        &mut warnings,
                                    )?;
                                }
                            }
                            _ => {}
                        }
                        deleted_item_ids.push(item.id.clone());
                    }
                }
            }
        }
    }

    let verified = adapter.scan(custom_home(&settings, kind)).await?;
    for session_id in &plan.expanded_session_ids {
        if verified
            .sessions
            .iter()
            .any(|session| &session.id == session_id)
        {
            return Err(CleanerError::Blocked(format!(
                "{} still lists session {session_id}; no private database repair was attempted",
                kind.display_name()
            )));
        }
    }
    *state.snapshot.lock() = Some(verified.clone());
    let reclaimed_bytes = before.saturating_sub(verified.total_bytes);
    write_journal(
        &state.data_dir,
        OperationJournal {
            operation_id: plan.id,
            status: OperationStatus::Complete,
            updated_at: Utc::now(),
            backup_id,
            message: None,
        },
    )?;

    Ok(CleanupResult {
        operation_id: plan.id,
        status: OperationStatus::Complete,
        backup_id,
        reclaimed_bytes,
        deleted_item_ids,
        warnings,
    })
}

#[tauri::command]
fn list_backups(state: State<'_, AppState>) -> CommandResult<Vec<BackupRecord>> {
    let retention = state.settings.lock().backup_retention_days;
    BackupStore::new(state.data_dir.join("backups"), retention)
        .and_then(|store| store.list())
        .map_err(error_message)
}

#[tauri::command]
async fn restore_backup(
    backup_id: Uuid,
    state: State<'_, AppState>,
) -> CommandResult<cleanerx_core::BackupManifest> {
    let settings = state.settings.lock().clone();
    let store = BackupStore::new(
        state.data_dir.join("backups"),
        settings.backup_retention_days,
    )
    .map_err(error_message)?;
    let record = store
        .list()
        .map_err(error_message)?
        .into_iter()
        .find(|record| record.id == backup_id)
        .ok_or_else(|| format!("Backup {backup_id} is not in the current catalog"))?;
    let kind = record.agent;
    let adapter = state.adapter(kind);
    let installation = adapter
        .detect(custom_home(&settings, kind))
        .await
        .map_err(error_message)?;
    if installation.running {
        return Err(format!(
            "Quit {} before restoring this backup",
            kind.display_name()
        ));
    }
    let home_label = match kind {
        AgentKind::Codex => "codex_home",
        AgentKind::ClaudeCode => "claude_home",
    };
    let mut roots = BTreeMap::from([(home_label.into(), PathBuf::from(&installation.home))]);
    if let Some(app_support) = &installation.app_support {
        roots.insert("codex_app_support".into(), PathBuf::from(app_support));
    }
    let manifest = store
        .restore(backup_id, kind, &roots)
        .map_err(error_message)?;
    let snapshot = adapter
        .scan(custom_home(&settings, kind))
        .await
        .map_err(error_message)?;
    *state.snapshot.lock() = Some(snapshot);
    Ok(manifest)
}

#[tauri::command]
fn purge_backup(backup_id: Uuid, state: State<'_, AppState>) -> CommandResult<()> {
    let retention = state.settings.lock().backup_retention_days;
    BackupStore::new(state.data_dir.join("backups"), retention)
        .and_then(|store| store.purge(backup_id))
        .map_err(error_message)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.settings.lock().clone()
}

#[tauri::command]
fn update_settings(
    settings: AppSettings,
    state: State<'_, AppState>,
) -> CommandResult<AppSettings> {
    validate_settings(&settings).map_err(error_message)?;
    save_json_atomic(&state.data_dir.join("settings.json"), &settings).map_err(error_message)?;
    *state.settings.lock() = settings.clone();
    Ok(settings)
}

fn validate_settings(settings: &AppSettings) -> Result<(), CleanerError> {
    if !matches!(settings.locale.as_str(), "system" | "zh" | "en") {
        return Err(CleanerError::InvalidRequest(
            "Unsupported interface language".into(),
        ));
    }
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err(CleanerError::InvalidRequest(
            "Unsupported appearance setting".into(),
        ));
    }
    if let Some(home) = &settings.custom_codex_home
        && !Path::new(home).is_absolute()
    {
        return Err(CleanerError::InvalidRequest(
            "Custom Codex home must be an absolute path".into(),
        ));
    }
    if let Some(home) = &settings.custom_claude_home
        && !Path::new(home).is_absolute()
    {
        return Err(CleanerError::InvalidRequest(
            "Custom Claude Code home must be an absolute path".into(),
        ));
    }
    if !(1..=3650).contains(&settings.backup_retention_days)
        || !(1..=365).contains(&settings.log_retention_days)
        || !(1..=24 * 365).contains(&settings.temp_retention_hours)
    {
        return Err(CleanerError::InvalidRequest(
            "Retention values are outside their safe range".into(),
        ));
    }
    Ok(())
}

fn custom_home(settings: &AppSettings, kind: AgentKind) -> Option<&str> {
    match kind {
        AgentKind::Codex => settings.custom_codex_home.as_deref(),
        AgentKind::ClaudeCode => settings.custom_claude_home.as_deref(),
    }
}

fn backup_sources(
    items: &[&cleanerx_core::CleanupItem],
    installation: &AgentInstallation,
) -> Result<Vec<BackupSource>, CleanerError> {
    let home = PathBuf::from(&installation.home);
    let app_support = installation.app_support.as_deref().map(PathBuf::from);
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for item in items.iter().filter(|item| item.recoverable) {
        for path in &item.paths {
            let path = PathBuf::from(path);
            if !path.exists() || !seen.insert(path.clone()) {
                continue;
            }
            if path.starts_with(&home) {
                sources.push(BackupSource {
                    root_label: match installation.kind {
                        AgentKind::Codex => "codex_home",
                        AgentKind::ClaudeCode => "claude_home",
                    }
                    .into(),
                    root_path: home.clone(),
                    path,
                });
            } else if let Some(app_support) = &app_support
                && path.starts_with(app_support)
            {
                sources.push(BackupSource {
                    root_label: "codex_app_support".into(),
                    root_path: app_support.clone(),
                    path,
                });
            }
        }
    }
    Ok(sources)
}

fn allowed_roots(installation: &AgentInstallation) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(&installation.home)];
    if let Some(app_support) = &installation.app_support {
        roots.push(PathBuf::from(app_support));
    }
    roots
}

fn protected_paths(installation: &AgentInstallation) -> Vec<PathBuf> {
    let home = Path::new(&installation.home);
    let names: &[&str] = match installation.kind {
        AgentKind::Codex => &[
            "auth.json",
            "config.toml",
            "rules",
            "skills",
            "plugins",
            "state",
            "installation_id",
        ],
        AgentKind::ClaudeCode => &[
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
        ],
    };
    names.iter().map(|name| home.join(name)).collect()
}

fn capture_mutation_identities(
    items: &[&cleanerx_core::CleanupItem],
    policy: &PathPolicy,
    kind: AgentKind,
) -> Result<HashMap<PathBuf, FileIdentity>, CleanerError> {
    let mut identities = HashMap::new();
    for item in items {
        if kind == AgentKind::ClaudeCode {
            validate_source_revision(item)?;
        }
        if kind == AgentKind::Codex
            && matches!(
                item.category,
                StorageCategory::Memory | StorageCategory::Log
            )
        {
            continue;
        }
        for path in &item.paths {
            let path = PathBuf::from(path);
            if path.exists() {
                let canonical = policy.validate_existing(&path)?;
                identities.insert(canonical.clone(), FileIdentity::capture(&canonical)?);
            }
        }
    }
    Ok(identities)
}

fn remove_operation_paths(
    operation: &cleanerx_core::PlannedOperation,
    selected_items: &[&cleanerx_core::CleanupItem],
    policy: &PathPolicy,
    identities: &HashMap<PathBuf, FileIdentity>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    for item_id in &operation.item_ids {
        let Some(item) = selected_items.iter().find(|item| &item.id == item_id) else {
            continue;
        };
        validate_source_revision(item)?;
        for path in &item.paths {
            remove_if_unchanged(Path::new(path), policy, identities, warnings)?;
        }
    }
    Ok(())
}

fn validate_source_revision(item: &cleanerx_core::CleanupItem) -> Result<(), CleanerError> {
    let expected = item.metadata.get("sourceRevision").ok_or_else(|| {
        CleanerError::UnsafePath(format!("missing scan revision for {}", item.id))
    })?;
    let paths: Vec<PathBuf> = item.paths.iter().map(PathBuf::from).collect();
    let current = cleanerx_core::metadata_revision(&paths)?;
    if &current != expected {
        return Err(CleanerError::UnsafePath(format!(
            "{} changed after the inventory scan",
            item.title
        )));
    }
    Ok(())
}

fn cleanup_session_artifacts(
    items: &[&cleanerx_core::CleanupItem],
    sessions: &[cleanerx_core::SessionRecord],
    policy: &PathPolicy,
    identities: &HashMap<PathBuf, FileIdentity>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    let rollout_paths: HashSet<PathBuf> = sessions
        .iter()
        .filter_map(|session| session.path.as_deref())
        .map(PathBuf::from)
        .collect();
    for item in items.iter().filter(|item| {
        matches!(
            item.category,
            StorageCategory::Session | StorageCategory::ArchivedSession
        )
    }) {
        for path in &item.paths {
            let path = PathBuf::from(path);
            if rollout_paths.contains(&path) {
                continue;
            }
            remove_if_unchanged(&path, policy, identities, warnings)?;
        }
    }
    Ok(())
}

fn remove_if_unchanged(
    path: &Path,
    policy: &PathPolicy,
    identities: &HashMap<PathBuf, FileIdentity>,
    warnings: &mut Vec<String>,
) -> Result<(), CleanerError> {
    if !path.exists() {
        return Ok(());
    }
    let canonical = policy.validate_existing(path)?;
    let identity = identities.get(&canonical).ok_or_else(|| {
        CleanerError::UnsafePath(format!("no preflight identity for {}", path.display()))
    })?;
    match safe_remove(&canonical, policy, Some(identity)) {
        Ok(_) => Ok(()),
        Err(error) => {
            warnings.push(format!("Skipped {}: {error}", path.display()));
            Err(error)
        }
    }
}

fn clean_logs(item: &cleanerx_core::CleanupItem, retention_days: u32) -> Result<(), CleanerError> {
    let cutoff = Utc::now().timestamp() - i64::from(retention_days) * 86_400;
    for path in item
        .paths
        .iter()
        .map(Path::new)
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sqlite"))
    {
        let connection = Connection::open(path)?;
        let columns = {
            let mut statement = connection.prepare("PRAGMA table_info(logs)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<HashSet<_>, _>>()?
        };
        if !columns.contains("ts") || !columns.contains("estimated_bytes") {
            return Err(CleanerError::Unsupported(format!(
                "unrecognized logs schema: {}",
                path.display()
            )));
        }
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let delete_result = connection.execute("DELETE FROM logs WHERE ts < ?1", [cutoff]);
        match delete_result {
            Ok(_) => connection.execute_batch("COMMIT")?,
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error.into());
            }
        }
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM")?;
    }
    Ok(())
}

fn write_journal(data_dir: &Path, journal: OperationJournal) -> Result<(), CleanerError> {
    save_json_atomic(
        &data_dir
            .join("operations")
            .join(format!("{}.json", journal.operation_id)),
        &journal,
    )
}

fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), CleanerError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("json.partial");
    let file = fs::File::create(&partial)?;
    serde_json::to_writer_pretty(file, value)?;
    fs::rename(partial, path)?;
    Ok(())
}

fn load_settings(data_dir: &Path) -> AppSettings {
    fs::File::open(data_dir.join("settings.json"))
        .ok()
        .and_then(|file| serde_json::from_reader(file).ok())
        .filter(|settings| validate_settings(settings).is_ok())
        .unwrap_or_default()
}

fn error_message(error: CleanerError) -> String {
    error.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_menu(Menu::default(app.handle())?)?;

            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let settings = load_settings(&data_dir);
            app.manage(AppState {
                codex_adapter: CodexAdapter::new(),
                claude_adapter: ClaudeCodeAdapter::new(),
                data_dir,
                settings: Mutex::new(settings),
                snapshot: Mutex::new(None),
                plans: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            detect_agents,
            scan_storage,
            get_session_projects,
            get_session_page,
            get_item_content,
            get_item_thumbnail,
            plan_cleanup,
            execute_cleanup,
            list_backups,
            restore_backup,
            purge_backup,
            get_settings,
            update_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running CleanerX");
}

#[cfg(test)]
mod tests {
    use super::{
        SessionFilter, SessionPageRequest, capture_mutation_identities, inventory_report,
        load_settings, remove_operation_paths, session_page, validate_settings,
        validate_source_revision,
    };
    use chrono::Utc;
    use cleanerx_core::{
        AgentCapabilities, AgentInstallation, AgentKind, AppSettings, CleanupItem,
        InventorySnapshot, OperationKind, PathPolicy, PlannedOperation, ProjectGroup, RiskLevel,
        SessionRecord, StorageCategory, metadata_revision,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn inventory_report_keeps_session_data_backend_only() {
        let snapshot = session_inventory_fixture();
        let report = inventory_report(&snapshot);

        assert!(report.items.iter().all(|item| item.thread_id.is_none()));
        assert_eq!(report.session_count, 3);
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].session_count, 2);
        assert_eq!(report.projects[0].size_bytes, 20);
        assert_eq!(report.unassigned_session_count, 1);
        assert_eq!(report.unassigned_session_size_bytes, 10);
    }

    #[test]
    fn session_pages_are_bounded_and_retain_filtered_ancestors() {
        let snapshot = session_inventory_fixture();
        let request = SessionPageRequest {
            filter: SessionFilter {
                snapshot_id: snapshot.id,
                project_id: Some("project".into()),
                query: "matching child".into(),
                ..SessionFilter::default()
            },
            cursor: 0,
            limit: 1,
            include_ancestors: true,
        };
        let page = session_page(&snapshot, &request).expect("bounded session page");

        assert_eq!(page.total_count, 1);
        assert_eq!(page.matching_session_ids, vec!["child"]);
        assert_eq!(
            page.sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "child"]
        );
        assert_eq!(page.items.len(), 2);

        let invalid = SessionPageRequest {
            limit: 101,
            ..request
        };
        assert!(session_page(&snapshot, &invalid).is_err());
    }

    #[test]
    fn validates_locale_and_theme_settings() {
        let settings = AppSettings::default();
        assert!(validate_settings(&settings).is_ok());

        let mut invalid_locale = settings.clone();
        invalid_locale.locale = "fr".into();
        assert!(validate_settings(&invalid_locale).is_err());

        let mut invalid_theme = settings;
        invalid_theme.theme = "neon".into();
        assert!(validate_settings(&invalid_theme).is_err());
    }

    #[test]
    fn invalid_persisted_preferences_fall_back_to_defaults() {
        let directory = tempdir().expect("temp settings directory");
        let settings = AppSettings {
            locale: "unsupported".into(),
            ..AppSettings::default()
        };
        fs::write(
            directory.path().join("settings.json"),
            serde_json::to_vec(&settings).expect("serialize settings"),
        )
        .expect("write settings");

        let loaded = load_settings(directory.path());
        assert_eq!(loaded.locale, "system");
        assert_eq!(loaded.theme, "system");
    }

    fn session_inventory_fixture() -> InventorySnapshot {
        let sessions = vec![
            test_session("root", "project root", None),
            test_session("child", "matching child", Some("root")),
            test_session("orphan", "unassigned", None),
        ];
        let items = sessions
            .iter()
            .map(|session| CleanupItem {
                id: format!("session:{}", session.id),
                category: StorageCategory::Session,
                title: session.name.clone(),
                subtitle: None,
                paths: session.path.clone().into_iter().collect(),
                project_id: (session.id != "orphan").then(|| "project".into()),
                thread_id: Some(session.id.clone()),
                size_bytes: session.size_bytes,
                modified_at: session.updated_at,
                risk: RiskLevel::High,
                recoverable: true,
                default_selected: false,
                protected: false,
                blocked_reason: None,
                metadata: BTreeMap::new(),
            })
            .collect();
        InventorySnapshot {
            id: Uuid::new_v4(),
            scanned_at: Utc::now(),
            installation: AgentInstallation {
                kind: AgentKind::Codex,
                home: "/tmp/.codex".into(),
                binary: None,
                version: Some("test".into()),
                app_support: None,
                running: false,
                capabilities: AgentCapabilities::default(),
                warnings: vec![],
            },
            total_bytes: 30,
            reclaimable_bytes: 0,
            items,
            sessions,
            projects: vec![ProjectGroup {
                id: "project".into(),
                name: "project".into(),
                roots: vec!["/tmp/project".into()],
                session_ids: vec!["root".into(), "child".into()],
                size_bytes: 20,
            }],
            categories: vec![],
            warnings: vec![],
        }
    }

    fn test_session(id: &str, name: &str, parent: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            name: name.into(),
            cwd: if id == "orphan" {
                String::new()
            } else {
                "/tmp/project".into()
            },
            path: Some(format!("/tmp/{id}.jsonl")),
            source: "cli".into(),
            archived: false,
            pinned: false,
            status: "notLoaded".into(),
            created_at: None,
            updated_at: None,
            size_bytes: 10,
            parent_thread_id: parent.map(str::to_owned),
            descendant_ids: vec![],
        }
    }

    #[test]
    fn claude_revision_change_blocks_cleanup_before_mutation() {
        let directory = tempdir().expect("Claude data");
        let transcript = directory.path().join("session.jsonl");
        fs::write(&transcript, b"first transcript metadata").expect("transcript");
        let item = claude_item(&transcript);
        assert!(validate_source_revision(&item).is_ok());

        fs::write(
            &transcript,
            b"replacement transcript metadata with a new size",
        )
        .expect("replacement");
        assert!(validate_source_revision(&item).is_err());
        assert!(transcript.exists());
    }

    #[test]
    fn claude_session_cleanup_preserves_configuration_and_source_bytes() {
        let home = tempdir().expect("Claude home");
        let source = tempdir().expect("source tree");
        let transcript = home.path().join("projects/project/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("transcript parent"))
            .expect("project bucket");
        fs::write(&transcript, b"session metadata").expect("transcript");
        let settings = home.path().join("settings.json");
        fs::write(&settings, b"protected settings").expect("settings");
        let source_file = source.path().join("source.rs");
        fs::write(&source_file, b"fn protected_source() {}").expect("source");
        let item = claude_item(&transcript);
        let selected = vec![&item];
        let policy = PathPolicy::new(vec![home.path().to_path_buf()], vec![settings.clone()]);
        let identities = capture_mutation_identities(&selected, &policy, AgentKind::ClaudeCode)
            .expect("preflight identities");
        let operation = PlannedOperation {
            kind: OperationKind::DeleteSession,
            item_ids: vec![item.id.clone()],
            session_ids: vec!["session".into()],
            paths: item.paths.clone(),
            size_bytes: item.size_bytes,
            backup_eligible: true,
            requires_agent_exit: true,
            blockers: vec![],
        };
        let mut warnings = Vec::new();

        remove_operation_paths(&operation, &selected, &policy, &identities, &mut warnings)
            .expect("remove session");

        assert!(!transcript.exists());
        assert_eq!(
            fs::read(settings).expect("settings bytes"),
            b"protected settings"
        );
        assert_eq!(
            fs::read(source_file).expect("source bytes"),
            b"fn protected_source() {}"
        );
        assert!(warnings.is_empty());
    }

    fn claude_item(path: &std::path::Path) -> CleanupItem {
        let paths = vec![path.to_path_buf()];
        CleanupItem {
            id: "session:session".into(),
            category: StorageCategory::Session,
            title: "Claude session".into(),
            subtitle: None,
            paths: paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            project_id: None,
            thread_id: Some("session".into()),
            size_bytes: fs::metadata(path).expect("metadata").len(),
            modified_at: None,
            risk: RiskLevel::High,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason: None,
            metadata: BTreeMap::from([(
                "sourceRevision".into(),
                metadata_revision(&paths).expect("revision"),
            )]),
        }
    }
}
