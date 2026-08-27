use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use adapter_codex::CodexAdapter;
use chrono::{DateTime, Utc};
use cleanerx_core::{
    AgentAdapter, AgentInstallation, AppSettings, BackupRecord, BackupSource, BackupStore,
    CleanerError, CleanupPlan, CleanupResult, FileIdentity, InventorySnapshot, ItemContentDetail,
    ItemThumbnail, OperationKind, OperationStatus, PathPolicy, StorageCategory,
    create_cleanup_plan, safe_remove,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::menu::Menu;
use tauri::{Manager, State};
use uuid::Uuid;

struct AppState {
    adapter: CodexAdapter,
    data_dir: PathBuf,
    settings: Mutex<AppSettings>,
    snapshot: Mutex<Option<InventorySnapshot>>,
    plans: Mutex<HashMap<Uuid, CleanupPlan>>,
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

type CommandResult<T> = Result<T, String>;

#[tauri::command]
async fn detect_agents(state: State<'_, AppState>) -> CommandResult<Vec<AgentInstallation>> {
    let custom_home = state.settings.lock().custom_codex_home.clone();
    state
        .adapter
        .detect(custom_home.as_deref())
        .await
        .map(|installation| vec![installation])
        .map_err(error_message)
}

#[tauri::command]
async fn scan_storage(state: State<'_, AppState>) -> CommandResult<InventorySnapshot> {
    let custom_home = state.settings.lock().custom_codex_home.clone();
    let snapshot = state
        .adapter
        .scan(custom_home.as_deref())
        .await
        .map_err(error_message)?;
    *state.snapshot.lock() = Some(snapshot.clone());
    Ok(snapshot)
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
        .adapter
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
        .adapter
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
    let current_installation = state
        .adapter
        .detect(settings.custom_codex_home.as_deref())
        .await?;
    if current_installation.running
        && plan
            .operations
            .iter()
            .any(|operation| operation.requires_codex_exit)
    {
        return Err(CleanerError::Blocked(
            "Quit Codex and retry this cleanup plan".into(),
        ));
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
    let identities = capture_mutation_identities(&selected_items, &policy)?;

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
                store.create_backup(plan, snapshot.installation.version.clone(), &sources)?;
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
                state
                    .adapter
                    .delete_sessions(&snapshot.installation, &operation.session_ids)
                    .await?;
                cleanup_session_artifacts(
                    &selected_items,
                    &snapshot.sessions,
                    &policy,
                    &identities,
                    &mut warnings,
                )?;
                deleted_item_ids.extend(operation.item_ids.iter().cloned());
            }
            OperationKind::ResetMemory => {
                state.adapter.reset_memory(&current_installation).await?;
                deleted_item_ids.extend(operation.item_ids.iter().cloned());
            }
            OperationKind::CleanRegenerable => {
                for item_id in &operation.item_ids {
                    let Some(item) = selected_items.iter().find(|item| &item.id == item_id) else {
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

    let verified = state
        .adapter
        .scan(settings.custom_codex_home.as_deref())
        .await?;
    for session_id in &plan.expanded_session_ids {
        if verified
            .sessions
            .iter()
            .any(|session| &session.id == session_id)
        {
            return Err(CleanerError::Blocked(format!(
                "Codex still lists session {session_id}; no direct database deletion was attempted"
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
    let installation = state
        .adapter
        .detect(settings.custom_codex_home.as_deref())
        .await
        .map_err(error_message)?;
    if installation.running {
        return Err("Quit Codex before restoring a backup".into());
    }
    let mut roots = BTreeMap::from([("codex_home".into(), PathBuf::from(&installation.home))]);
    if let Some(app_support) = &installation.app_support {
        roots.insert("codex_app_support".into(), PathBuf::from(app_support));
    }
    let store = BackupStore::new(
        state.data_dir.join("backups"),
        settings.backup_retention_days,
    )
    .map_err(error_message)?;
    let manifest = store.restore(backup_id, &roots).map_err(error_message)?;
    let snapshot = state
        .adapter
        .scan(settings.custom_codex_home.as_deref())
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
                    root_label: "codex_home".into(),
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
    [
        "auth.json",
        "config.toml",
        "rules",
        "skills",
        "plugins",
        "state",
        "installation_id",
    ]
    .map(|name| home.join(name))
    .into_iter()
    .collect()
}

fn capture_mutation_identities(
    items: &[&cleanerx_core::CleanupItem],
    policy: &PathPolicy,
) -> Result<HashMap<PathBuf, FileIdentity>, CleanerError> {
    let mut identities = HashMap::new();
    for item in items {
        if matches!(
            item.category,
            StorageCategory::Memory | StorageCategory::Log
        ) {
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
                adapter: CodexAdapter::new(),
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
    use super::{load_settings, validate_settings};
    use cleanerx_core::AppSettings;
    use std::fs;
    use tempfile::tempdir;

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
}
