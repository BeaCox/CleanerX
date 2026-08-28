use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::Utc;
use uuid::Uuid;

use crate::{
    CleanerError, CleanupItem, CleanupPlan, InventorySnapshot, OperationKind, PlannedOperation,
    StorageCategory,
};

/// Builds an immutable cleanup plan from a specific inventory snapshot.
/// Session descendants are always included so the UI can disclose the full impact of
/// an Agent's cascading session-deletion behavior before execution.
pub fn create_cleanup_plan(
    snapshot: &InventorySnapshot,
    selected_item_ids: &[String],
) -> Result<CleanupPlan, CleanerError> {
    if selected_item_ids.is_empty() {
        return Err(CleanerError::InvalidRequest(
            "select at least one cleanup item".into(),
        ));
    }
    let item_by_id: HashMap<&str, &CleanupItem> = snapshot
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut selected = BTreeSet::new();
    for item_id in selected_item_ids {
        let item = item_by_id
            .get(item_id.as_str())
            .ok_or_else(|| CleanerError::NotFound(format!("cleanup item {item_id}")))?;
        if item.protected || item.category == StorageCategory::Protected {
            return Err(CleanerError::Blocked(format!(
                "protected item cannot be selected: {item_id}"
            )));
        }
        selected.insert(item_id.clone());
    }

    let session_by_id: HashMap<&str, _> = snapshot
        .sessions
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect();
    let selected_session_ids: HashSet<String> = selected
        .iter()
        .filter_map(|item_id| {
            let item = item_by_id[item_id.as_str()];
            matches!(
                item.category,
                StorageCategory::Session | StorageCategory::ArchivedSession
            )
            .then(|| item.thread_id.clone())
            .flatten()
        })
        .collect();
    let mut expanded_sessions = BTreeSet::new();
    for session_id in &selected_session_ids {
        expanded_sessions.insert(session_id.clone());
        if let Some(session) = session_by_id.get(session_id.as_str()) {
            expanded_sessions.extend(session.descendant_ids.iter().cloned());
        }
    }

    // If both an ancestor and its child are selected, only request deletion of the
    // ancestor. The selected Agent deletes the descendant as part of that operation.
    let deletion_roots: Vec<String> = selected_session_ids
        .iter()
        .filter(|candidate| {
            !selected_session_ids.iter().any(|possible_ancestor| {
                possible_ancestor != *candidate
                    && session_by_id
                        .get(possible_ancestor.as_str())
                        .is_some_and(|session| session.descendant_ids.contains(candidate))
            })
        })
        .cloned()
        .collect();

    let mut grouped: BTreeMap<OperationKind, PlannedOperation> = BTreeMap::new();
    for item_id in &selected {
        let item = item_by_id[item_id.as_str()];
        let kind = operation_kind(item.category);
        add_to_operation(&mut grouped, kind, item);
    }
    if !deletion_roots.is_empty() {
        let operation = grouped
            .entry(OperationKind::DeleteSession)
            .or_insert_with(|| blank_operation(OperationKind::DeleteSession));
        operation.item_ids.clear();
        operation.paths.clear();
        operation.size_bytes = 0;
        operation.session_ids = deletion_roots;
        operation.backup_eligible = false;
        for session_id in &expanded_sessions {
            if let Some(session) = session_by_id.get(session_id.as_str()) {
                operation.size_bytes = operation.size_bytes.saturating_add(session.size_bytes);
                if let Some(path) = &session.path
                    && !operation.paths.contains(path)
                {
                    operation.paths.push(path.clone());
                }
                let item_id = format!("session:{session_id}");
                if let Some(item) = item_by_id.get(item_id.as_str()) {
                    operation.backup_eligible |= item.recoverable;
                    if !operation.item_ids.contains(&item_id) {
                        operation.item_ids.push(item_id);
                    }
                }
            }
        }
    }

    let mut blockers = Vec::new();
    for item_id in &selected {
        let item = item_by_id[item_id.as_str()];
        if let Some(reason) = &item.blocked_reason {
            blockers.push(format!("{}: {reason}", item.title));
        }
        if matches!(
            item.category,
            StorageCategory::Attachment | StorageCategory::GeneratedImage
        ) {
            blockers.push(format!(
                "{}: session-owned media can be removed only after its owning session deletion succeeds",
                item.title
            ));
        }
    }
    for session_id in &expanded_sessions {
        if let Some(session) = session_by_id.get(session_id.as_str()) {
            if session.pinned {
                blockers.push(format!("Session '{}' is pinned", session.name));
            }
            let status = session.status.to_ascii_lowercase();
            if matches!(status.as_str(), "active" | "loaded") {
                blockers.push(format!("Session '{}' is active or loaded", session.name));
            }
        }
    }
    if !selected_session_ids.is_empty()
        && (!snapshot.installation.capabilities.thread_delete
            || snapshot.installation.capabilities.report_only)
    {
        blockers.push(format!(
            "{} does not expose a supported session deletion route",
            snapshot.installation.kind.display_name()
        ));
    }
    if selected
        .iter()
        .any(|item_id| item_by_id[item_id.as_str()].category == StorageCategory::Memory)
        && !(snapshot.installation.capabilities.memory.can_reset_scope
            || snapshot.installation.capabilities.memory.can_delete_entries)
    {
        blockers.push(format!(
            "{} does not expose a supported memory deletion route",
            snapshot.installation.kind.display_name()
        ));
    }
    blockers.sort();
    blockers.dedup();
    for operation in grouped.values_mut() {
        operation.blockers = blockers.clone();
    }

    let estimated_bytes = grouped.values().map(|operation| operation.size_bytes).sum();
    let backup_covers_full_plan = grouped
        .values()
        .flat_map(|operation| &operation.item_ids)
        .all(|item_id| {
            item_by_id
                .get(item_id.as_str())
                .is_some_and(|item| item.recoverable)
        });
    if !backup_covers_full_plan {
        for operation in grouped.values_mut() {
            operation.backup_eligible = false;
        }
    }
    let estimated_backup_bytes = if backup_covers_full_plan {
        estimated_bytes
    } else {
        0
    };

    Ok(CleanupPlan {
        id: Uuid::new_v4(),
        snapshot_id: snapshot.id,
        created_at: Utc::now(),
        selected_item_ids: selected.into_iter().collect(),
        expanded_session_ids: expanded_sessions.into_iter().collect(),
        operations: grouped.into_values().collect(),
        estimated_bytes,
        estimated_backup_bytes,
        blockers,
    })
}

fn operation_kind(category: StorageCategory) -> OperationKind {
    match category {
        StorageCategory::Session | StorageCategory::ArchivedSession => OperationKind::DeleteSession,
        StorageCategory::Memory => OperationKind::ResetMemory,
        StorageCategory::Attachment
        | StorageCategory::GeneratedImage
        | StorageCategory::Log
        | StorageCategory::Cache
        | StorageCategory::Temporary
        | StorageCategory::Protected => OperationKind::CleanRegenerable,
    }
}

fn blank_operation(kind: OperationKind) -> PlannedOperation {
    PlannedOperation {
        kind,
        item_ids: Vec::new(),
        session_ids: Vec::new(),
        paths: Vec::new(),
        size_bytes: 0,
        backup_eligible: false,
        requires_agent_exit: false,
        blockers: Vec::new(),
    }
}

fn add_to_operation(
    grouped: &mut BTreeMap<OperationKind, PlannedOperation>,
    kind: OperationKind,
    item: &CleanupItem,
) {
    let operation = grouped.entry(kind).or_insert_with(|| blank_operation(kind));
    operation.item_ids.push(item.id.clone());
    operation.paths.extend(item.paths.iter().cloned());
    operation.size_bytes = operation.size_bytes.saturating_add(item.size_bytes);
    operation.backup_eligible |= item.recoverable;
    operation.requires_agent_exit |= matches!(
        item.category,
        StorageCategory::Memory
            | StorageCategory::Log
            | StorageCategory::Cache
            | StorageCategory::Temporary
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentCapabilities, AgentInstallation, AgentKind, InventorySnapshot, RiskLevel,
        SessionRecord,
    };

    #[test]
    fn expands_descendants_and_keeps_only_root_delete() {
        let mut snapshot = fixture();
        snapshot.sessions[0].descendant_ids = vec!["child".into()];
        let plan = create_cleanup_plan(&snapshot, &["session:root".into(), "session:child".into()])
            .expect("plan");
        assert_eq!(plan.expanded_session_ids, vec!["child", "root"]);
        assert_eq!(plan.operations[0].session_ids, vec!["root"]);
        assert!(!plan.operations[0].requires_agent_exit);
    }

    #[test]
    fn refuses_protected_data() {
        let mut snapshot = fixture();
        snapshot.items[0].protected = true;
        assert!(create_cleanup_plan(&snapshot, &["session:root".into()]).is_err());
    }

    #[test]
    fn media_selection_never_expands_into_an_owning_session_delete() {
        let mut snapshot = fixture();
        snapshot.items.push(CleanupItem {
            id: "attachment:root".into(),
            category: StorageCategory::Attachment,
            title: "Root attachment".into(),
            subtitle: None,
            paths: vec!["/tmp/attachments/root".into()],
            project_id: None,
            thread_id: Some("root".into()),
            size_bytes: 1,
            modified_at: None,
            risk: RiskLevel::Review,
            recoverable: true,
            default_selected: false,
            protected: false,
            blocked_reason: None,
            metadata: BTreeMap::new(),
        });

        let plan = create_cleanup_plan(&snapshot, &["attachment:root".into()]).expect("plan");

        assert!(plan.expanded_session_ids.is_empty());
        assert!(
            plan.operations
                .iter()
                .all(|operation| operation.kind != OperationKind::DeleteSession)
        );
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("only after its owning session deletion succeeds"))
        );
    }

    #[test]
    fn session_backup_eligibility_comes_from_the_adapter_item() {
        let mut snapshot = fixture();
        snapshot.items[0].recoverable = false;

        let plan = create_cleanup_plan(&snapshot, &["session:root".into()]).expect("plan");

        assert!(!plan.operations[0].backup_eligible);
        assert_eq!(plan.estimated_backup_bytes, 0);
    }

    #[test]
    fn pinned_sessions_remain_blocked_at_the_core_boundary() {
        let mut snapshot = fixture();
        snapshot.sessions[0].pinned = true;

        let plan = create_cleanup_plan(&snapshot, &["session:root".into()]).expect("plan");

        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("is pinned"))
        );
    }

    #[test]
    fn backup_is_not_offered_for_a_partially_restorable_plan() {
        let mut snapshot = fixture();
        snapshot.items.push(CleanupItem {
            id: "cache:test".into(),
            category: StorageCategory::Cache,
            title: "Regenerable cache".into(),
            subtitle: None,
            paths: vec!["/tmp/cache".into()],
            project_id: None,
            thread_id: None,
            size_bytes: 5,
            modified_at: None,
            risk: RiskLevel::Safe,
            recoverable: false,
            default_selected: false,
            protected: false,
            blocked_reason: None,
            metadata: BTreeMap::new(),
        });

        let plan = create_cleanup_plan(&snapshot, &["session:root".into(), "cache:test".into()])
            .expect("plan");

        assert_eq!(plan.estimated_backup_bytes, 0);
        assert!(
            plan.operations
                .iter()
                .all(|operation| !operation.backup_eligible)
        );
    }

    fn fixture() -> InventorySnapshot {
        let sessions = vec![session("root", None), session("child", Some("root"))];
        let items = sessions
            .iter()
            .map(|session| CleanupItem {
                id: format!("session:{}", session.id),
                category: StorageCategory::Session,
                title: session.name.clone(),
                subtitle: None,
                paths: session.path.clone().into_iter().collect(),
                project_id: None,
                thread_id: Some(session.id.clone()),
                size_bytes: 10,
                modified_at: None,
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
                state: Default::default(),
                home: "/tmp/.codex".into(),
                binary: Some("codex".into()),
                version: Some("test".into()),
                app_support: None,
                running: false,
                capabilities: AgentCapabilities {
                    thread_list: true,
                    thread_delete: true,
                    memory: crate::MemoryCapabilities {
                        can_scan: true,
                        can_read_content: true,
                        can_reset_all: true,
                        can_reset_scope: true,
                        ..crate::MemoryCapabilities::default()
                    },
                    descendant_filter: true,
                    report_only: false,
                },
                warnings: vec![],
            },
            total_bytes: 20,
            reclaimable_bytes: 0,
            items,
            sessions,
            projects: vec![],
            categories: vec![],
            warnings: vec![],
        }
    }

    fn session(id: &str, parent: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            name: id.into(),
            cwd: "/tmp/project".into(),
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
}
