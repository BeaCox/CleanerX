use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, State, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};

const MAX_RELEASE_NOTES_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AppUpdateSupport {
    Available,
    LinuxPackage,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateMetadata {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    current_version: String,
    support: AppUpdateSupport,
    update: Option<AppUpdateMetadata>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum AppUpdateEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

#[derive(Default)]
pub struct PendingAppUpdate(Mutex<Option<Update>>);

#[tauri::command]
pub fn get_app_update_status(app: AppHandle) -> AppUpdateStatus {
    status_for(
        app.package_info().version.to_string(),
        current_support(),
        None,
    )
}

#[tauri::command]
pub async fn check_for_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingAppUpdate>,
) -> Result<AppUpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    let support = current_support();

    clear_pending_update(&pending_update)?;
    if support != AppUpdateSupport::Available {
        return Ok(status_for(current_version, support, None));
    }

    let update = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;
    let metadata = update.as_ref().map(metadata_from_update);
    *pending_update
        .0
        .lock()
        .map_err(|_| "The pending application update lock is unavailable".to_owned())? = update;

    Ok(status_for(current_version, support, metadata))
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingAppUpdate>,
    on_event: Channel<AppUpdateEvent>,
) -> Result<(), String> {
    let update = pending_update
        .0
        .lock()
        .map_err(|_| "The pending application update lock is unavailable".to_owned())?
        .take()
        .ok_or_else(|| "There is no verified application update ready to install".to_owned())?;
    let mut started = false;

    update
        .download_and_install(
            |chunk_length, content_length| {
                if !started {
                    let _ = on_event.send(AppUpdateEvent::Started { content_length });
                    started = true;
                }
                let _ = on_event.send(AppUpdateEvent::Progress { chunk_length });
            },
            || {
                let _ = on_event.send(AppUpdateEvent::Finished);
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    {
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        app.restart()
    }
}

fn clear_pending_update(pending_update: &PendingAppUpdate) -> Result<(), String> {
    pending_update
        .0
        .lock()
        .map_err(|_| "The pending application update lock is unavailable".to_owned())?
        .take();
    Ok(())
}

fn current_support() -> AppUpdateSupport {
    support_for(std::env::consts::OS, std::env::var_os("APPIMAGE").is_some())
}

fn support_for(target_os: &str, running_as_appimage: bool) -> AppUpdateSupport {
    match target_os {
        "macos" | "windows" => AppUpdateSupport::Available,
        "linux" if running_as_appimage => AppUpdateSupport::Available,
        "linux" => AppUpdateSupport::LinuxPackage,
        _ => AppUpdateSupport::UnsupportedPlatform,
    }
}

fn metadata_from_update(update: &Update) -> AppUpdateMetadata {
    AppUpdateMetadata {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.as_deref().map(bounded_release_notes),
        date: update.date.map(|date| date.to_string()),
    }
}

fn bounded_release_notes(notes: &str) -> String {
    notes.chars().take(MAX_RELEASE_NOTES_CHARS).collect()
}

fn status_for(
    current_version: String,
    support: AppUpdateSupport,
    update: Option<AppUpdateMetadata>,
) -> AppUpdateStatus {
    AppUpdateStatus {
        current_version,
        support,
        update,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppUpdateSupport, MAX_RELEASE_NOTES_CHARS, bounded_release_notes, status_for, support_for,
    };

    #[test]
    fn native_installers_and_appimages_support_in_app_updates() {
        assert_eq!(support_for("macos", false), AppUpdateSupport::Available);
        assert_eq!(support_for("windows", false), AppUpdateSupport::Available);
        assert_eq!(support_for("linux", true), AppUpdateSupport::Available);
    }

    #[test]
    fn linux_packages_require_their_distribution_channel() {
        assert_eq!(support_for("linux", false), AppUpdateSupport::LinuxPackage);
        assert_eq!(
            support_for("freebsd", false),
            AppUpdateSupport::UnsupportedPlatform
        );
    }

    #[test]
    fn local_status_does_not_claim_that_a_network_check_ran() {
        let status = status_for("0.1.0-alpha.1".into(), AppUpdateSupport::Available, None);
        assert_eq!(status.current_version, "0.1.0-alpha.1");
        assert_eq!(status.support, AppUpdateSupport::Available);
        assert!(status.update.is_none());
    }

    #[test]
    fn release_notes_are_bounded_before_reaching_the_webview() {
        let notes = "x".repeat(MAX_RELEASE_NOTES_CHARS + 50);
        assert_eq!(
            bounded_release_notes(&notes).chars().count(),
            MAX_RELEASE_NOTES_CHARS
        );
    }
}
