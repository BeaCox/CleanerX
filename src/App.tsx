import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  Bot,
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronsDown,
  ChevronsUp,
  CircleAlert,
  Database,
  FolderCode,
  GitBranch,
  HardDrive,
  Image,
  List,
  LoaderCircle,
  LockKeyhole,
  MemoryStick,
  Monitor,
  Moon,
  Pin,
  RefreshCw,
  RotateCcw,
  Search,
  Settings,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import { api } from "./api";
import i18n from "./i18n";
import { applyPreferences, watchSystemPreferences } from "./preferences";
import { cachePreferences } from "./preferenceStore";
import type {
  AppSettings,
  BackupRecord,
  CleanupItem,
  CleanupPlan,
  ContentBlock,
  InventorySnapshot,
  ItemContentDetail,
  ProjectGroup,
  SessionRecord,
  StorageCategory,
  ViewId,
} from "./types";

const categoryColors: Record<StorageCategory, string> = {
  session: "#3f6f9f",
  archivedSession: "#7d8794",
  memory: "#a65d73",
  attachment: "#b47a36",
  generatedImage: "#7653a6",
  log: "#3f8f9d",
  cache: "#4e8b64",
  temporary: "#899443",
  protected: "#8a8a84",
};

const categoryTranslation: Record<StorageCategory, string> = {
  session: "categorySession",
  archivedSession: "categoryArchivedSession",
  memory: "categoryMemory",
  attachment: "categoryAttachment",
  generatedImage: "categoryGeneratedImage",
  log: "categoryLog",
  cache: "categoryCache",
  temporary: "categoryTemporary",
  protected: "categoryProtected",
};

const NO_PROJECT_ID = "__no_project";

const navItems: Array<{ id: ViewId; icon: typeof HardDrive; label: string }> = [
  { id: "overview", icon: HardDrive, label: "overview" },
  { id: "sessions", icon: Bot, label: "sessions" },
  { id: "memory", icon: MemoryStick, label: "memory" },
  { id: "generated", icon: Image, label: "generated" },
  { id: "logs", icon: Database, label: "logs" },
  { id: "backups", icon: Archive, label: "backups" },
  { id: "settings", icon: Settings, label: "settings" },
];

export default function App() {
  const { t } = useTranslation();
  const [view, setView] = useState<ViewId>("overview");
  const [snapshot, setSnapshot] = useState<InventorySnapshot>();
  const [backups, setBackups] = useState<BackupRecord[]>([]);
  const [settings, setSettings] = useState<AppSettings>();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [plan, setPlan] = useState<CleanupPlan>();
  const [createBackup, setCreateBackup] = useState(false);
  const [busy, setBusy] = useState<"scan" | "plan" | "execute" | "restore" | "purge" | null>("scan");
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [detailItemId, setDetailItemId] = useState<string>();
  const [backupToPurge, setBackupToPurge] = useState<BackupRecord>();
  const loaded = useRef(false);

  const scan = useCallback(async () => {
    setBusy("scan");
    setError(undefined);
    try {
      const result = await api.scanStorage();
      setSnapshot(result);
      setSelected((current) => new Set(
        [...current].filter((id) => result.items.some((item) => item.id === id && isItemSelectable(item, result))),
      ));
      setNotice(t("scanComplete"));
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(null);
    }
  }, [t]);

  useEffect(() => {
    if (loaded.current) return;
    loaded.current = true;
    void Promise.all([api.getSettings(), api.listBackups()])
      .then(([nextSettings, nextBackups]) => {
        cachePreferences(nextSettings);
        setSettings(nextSettings);
        setBackups(nextBackups);
      })
      .catch((reason) => setError(messageOf(reason)));
    void scan();
  }, [scan]);

  useEffect(() => {
    if (!notice) return;
    const timeout = window.setTimeout(() => setNotice(undefined), 2800);
    return () => window.clearTimeout(timeout);
  }, [notice]);

  useEffect(() => {
    if (!settings) return;
    void applyPreferences(settings);
    if (view === "settings") return;
    return watchSystemPreferences(settings);
  }, [settings, view]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "r") {
        event.preventDefault();
        void scan();
      }
      if (event.key === "Escape") {
        setPlan(undefined);
        setDetailItemId(undefined);
        setBackupToPurge(undefined);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [scan]);

  const selectedItems = useMemo(
    () => snapshot?.items.filter((item) => selected.has(item.id)) ?? [],
    [selected, snapshot],
  );
  const selectedBytes = selectedItems.reduce((sum, item) => sum + item.sizeBytes, 0);

  const toggleItem = (item: CleanupItem) => {
    if (!snapshot || !isItemSelectable(item, snapshot)) return;
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(item.id)) next.delete(item.id);
      else next.add(item.id);
      return next;
    });
  };

  const selectMany = useCallback((items: CleanupItem[], shouldSelect: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      items.forEach((item) => {
        if (!snapshot || !isItemSelectable(item, snapshot)) return;
        if (shouldSelect) next.add(item.id); else next.delete(item.id);
      });
      return next;
    });
  }, [snapshot]);

  const reviewPlan = async () => {
    setBusy("plan");
    setError(undefined);
    try {
      const nextPlan = await api.planCleanup([...selected]);
      setPlan(nextPlan);
      setCreateBackup(false);
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(null);
    }
  };

  const executePlan = async () => {
    if (!plan || plan.blockers.length) return;
    setBusy("execute");
    setError(undefined);
    try {
      const result = await api.executeCleanup(plan.id, createBackup);
      setPlan(undefined);
      setSelected(new Set());
      setNotice(`${t("cleanupComplete")} · ${formatBytes(result.reclaimedBytes)}`);
      await scan();
      setBackups(await api.listBackups());
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(null);
    }
  };

  const restore = async (backupId: string) => {
    setBusy("restore");
    setError(undefined);
    try {
      await api.restoreBackup(backupId);
      setNotice(t("restore"));
      await scan();
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(null);
    }
  };

  const purge = async () => {
    if (!backupToPurge) return;
    setBusy("purge");
    setError(undefined);
    try {
      await api.purgeBackup(backupToPurge.id);
      setBackups(await api.listBackups());
      setBackupToPurge(undefined);
      setNotice(t("backupDeleted"));
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(null);
    }
  };

  const saveSettings = async (next: AppSettings) => {
    try {
      const saved = await api.updateSettings(next);
      cachePreferences(saved);
      await applyPreferences(saved);
      setSettings(saved);
      setNotice(i18n.t("settingsSaved"));
    } catch (reason) {
      setError(messageOf(reason));
    }
  };

  const storageView = view === "overview" || view === "sessions" || view === "memory" || view === "generated" || view === "logs";
  const detailItem = snapshot?.items.find((item) => item.id === detailItemId);

  return (
    <div className="app-shell">
      <header className="app-toolbar">
        <div className="toolbar-brand">
          <span className="brand-mark" aria-hidden="true">CX</span>
          <strong>{t("appName")}</strong>
        </div>
        <nav className="view-tabs" aria-label="Primary navigation">
          {navItems.map(({ id, icon: Icon, label }) => (
            <button
              key={id}
              className={view === id ? "tab-active" : ""}
              aria-current={view === id ? "page" : undefined}
              onClick={() => setView(id)}
            >
              <Icon size={14} />
              <span>{t(label)}</span>
              {id === "sessions" && snapshot && <small>{snapshot.sessions.length}</small>}
              {id === "backups" && backups.length > 0 && <small>{backups.length}</small>}
            </button>
          ))}
        </nav>
        <div className="toolbar-actions">
          {storageView && <button className="secondary-button" onClick={() => void scan()} disabled={busy !== null}>
            <RefreshCw size={14} className={busy === "scan" ? "spinning" : ""} />
            {busy === "scan" ? t("scanning") : t("scan")}
          </button>}
        </div>
      </header>

      <main className="content-scroll">
        <h1 className="visually-hidden">{t(navItems.find((item) => item.id === view)?.label ?? "overview")}</h1>
        {error && <div className="alert alert-error" role="alert"><CircleAlert size={18} /><span>{error}</span><button onClick={() => setError(undefined)}><X size={16} /></button></div>}
        {storageView && snapshot?.installation.capabilities.reportOnly && <div className="alert alert-warning capability-alert"><CircleAlert size={18} /><div><strong>{t("reportOnlyNotice")}</strong><span>{t("reportOnlyHelp")}</span>{snapshot.installation.warnings[0] && <code>{snapshot.installation.warnings[0]}</code>}</div><button className="secondary-button" onClick={() => void scan()} disabled={busy !== null}>{t("retryConnection")}</button></div>}
        {view === "settings" ? (
          settings ? <SettingsView value={settings} onSave={saveSettings} /> : <LoadingState label={t("loadingSettings")} icon={Settings} />
        ) : view === "backups" ? (
          <BackupsView backups={backups} restore={restore} requestPurge={setBackupToPurge} busy={busy !== null} />
        ) : !snapshot ? (
          <StorageLoadingView view={view} failed={busy !== "scan"} />
        ) : (
          <>
            {view === "overview" && <Overview snapshot={snapshot} />}
            {view === "sessions" && <SessionsView snapshot={snapshot} selected={selected} toggle={toggleItem} selectMany={selectMany} inspect={(item) => setDetailItemId(item.id)} />}
            {view === "memory" && <MemoryView snapshot={snapshot} selected={selected} toggle={toggleItem} selectMany={selectMany} inspect={(item) => setDetailItemId(item.id)} />}
            {view === "generated" && <MediaView snapshot={snapshot} selected={selected} toggle={toggleItem} selectMany={selectMany} inspect={(item) => setDetailItemId(item.id)} />}
            {view === "logs" && <ItemsView snapshot={snapshot} selected={selected} toggle={toggleItem} selectMany={selectMany} inspect={(item) => setDetailItemId(item.id)} categories={["log", "cache", "temporary", "protected"]} />}
          </>
        )}
      </main>

      <footer className="status-bar">
        <span className="status-env">
          <span className={`status-dot ${snapshot?.installation.capabilities.reportOnly ? "status-warning" : ""}`} />
          Codex {snapshot?.installation.version?.replace("codex-cli ", "") ?? "—"}
        </span>
        {selected.size > 0 && !plan ? (
          <span className="status-selection" role="status">
            <strong>{selected.size} {t("selected")}</strong>
            <span>{formatBytes(selectedBytes)}</span>
            <button className="text-button" onClick={() => setSelected(new Set())}>{t("clearSelection")}</button>
            <button className="primary-button status-cta" onClick={() => void reviewPlan()} disabled={busy !== null}>
              {busy === "plan" && <LoaderCircle size={13} className="spinning" />}
              {t("reviewCleanup")}<ChevronRight size={13} />
            </button>
          </span>
        ) : snapshot ? (
          <span className="status-inventory">
            {t("lastScan")} {relativeTime(snapshot.scannedAt, i18n.language)} · {snapshot.categories.reduce((sum, category) => sum + category.itemCount, 0)} {t("items")} · {formatBytes(snapshot.totalBytes)}
          </span>
        ) : (
          <span className="status-inventory">{t("waitingForScanData")}</span>
        )}
      </footer>

      {plan && (
        <ReviewDialog
          plan={plan}
          snapshot={snapshot!}
          createBackup={createBackup}
          setCreateBackup={setCreateBackup}
          close={() => setPlan(undefined)}
          execute={() => void executePlan()}
          executing={busy === "execute"}
        />
      )}
      {detailItem && snapshot && <ItemDetailDialog item={detailItem} snapshot={snapshot} selected={selected.has(detailItem.id)} toggle={() => toggleItem(detailItem)} close={() => setDetailItemId(undefined)} />}
      {backupToPurge && <PurgeBackupDialog backup={backupToPurge} close={() => setBackupToPurge(undefined)} purge={() => void purge()} purging={busy === "purge"} />}
      {notice && <div className="toast"><Check size={16} />{notice}</div>}
    </div>
  );
}

function Overview({ snapshot }: { snapshot: InventorySnapshot }) {
  const { t } = useTranslation();
  const categories = snapshot.categories.filter((item) => item.category !== "protected" && item.sizeBytes > 0);
  const chartCategories = categories.map((summary) => ({ summary, color: categoryColors[summary.category] }));
  const itemCount = snapshot.categories.reduce((sum, category) => sum + category.itemCount, 0);
  return (
    <div className="page-stack">
      <section className="stat-strip">
        <Stat value={formatBytes(snapshot.totalBytes)} label={t("totalManaged")} detail={`${itemCount} ${t("items")}`} />
        <Stat value={String(snapshot.projects.length)} label={t("projectsCount")} />
        <Stat value={String(snapshot.sessions.length)} label={t("sessionsCount")} detail={`${snapshot.sessions.filter((session) => session.archived).length} ${t("archived")}`} />
      </section>
      <section className="panel-card">
        <div className="panel-heading"><h3>{t("storageBreakdown")}</h3><span>{itemCount} {t("items")}</span></div>
        <div className="overview-chart-layout">
          <StorageDonut categories={chartCategories} totalBytes={snapshot.totalBytes} />
          <div className="category-list">
            {chartCategories.map(({ summary, color }) => (
              <div className="category-row" data-category={summary.category} key={summary.category} style={{ "--category-color": color } as CSSProperties}>
                <span className="category-swatch" />
                <div><strong>{t(categoryTranslation[summary.category])}</strong><span>{summary.itemCount} {t("items")}</span></div>
                <div className="category-track"><span style={{ width: `${Math.max(4, summary.sizeBytes / snapshot.totalBytes * 100)}%` }} /></div>
                <strong>{formatBytes(summary.sizeBytes)}</strong>
              </div>
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}

function StorageDonut({ categories, totalBytes }: { categories: Array<{ summary: InventorySnapshot["categories"][number]; color: string }>; totalBytes: number }) {
  const { t } = useTranslation();
  const radius = 62;
  const circumference = 2 * Math.PI * radius;
  let offset = 0;
  return <figure className="storage-donut-figure">
    <div className="storage-donut-shell">
      <svg viewBox="0 0 160 160" role="img" aria-label={`${t("storageChartLabel")} · ${formatBytes(totalBytes)}`}>
        <circle className="storage-donut-track" cx="80" cy="80" r={radius} />
        {categories.map(({ summary, color }) => {
          const length = totalBytes > 0 ? summary.sizeBytes / totalBytes * circumference : 0;
          const segment = <circle
            className="storage-donut-segment"
            data-category={summary.category}
            key={summary.category}
            cx="80"
            cy="80"
            r={radius}
            style={{ "--category-color": color } as CSSProperties}
            strokeDasharray={`${length} ${circumference - length}`}
            strokeDashoffset={-offset}
          />;
          offset += length;
          return segment;
        })}
      </svg>
      <div className="storage-donut-center"><strong>{formatBytes(totalBytes)}</strong><span>{t("storageChartTotal")}</span></div>
    </div>
  </figure>;
}

function Stat({ value, label, detail }: { value: string; label: string; detail?: string }) {
  return <div className="stat"><span>{label}</span><strong>{value}</strong>{detail && <small>{detail}</small>}</div>;
}

interface SelectionProps {
  snapshot: InventorySnapshot;
  selected: Set<string>;
  toggle: (item: CleanupItem) => void;
  selectMany: (items: CleanupItem[], shouldSelect: boolean) => void;
  inspect: (item: CleanupItem) => void;
}

function SessionsView({ snapshot, selected, toggle, selectMany, inspect }: SelectionProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [project, setProject] = useState("all");
  const [source, setSource] = useState("all");
  const [state, setState] = useState("all");
  const [updatedWithin, setUpdatedWithin] = useState("all");
  const [displayMode, setDisplayMode] = useState<"tree" | "list">("tree");
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(sessionTreeExpansionKeys(snapshot, new Set(snapshot.sessions.map((session) => session.id)))));
  const hasNoProjectSessions = snapshot.sessions.some((session) => !snapshot.items.find((candidate) => candidate.threadId === session.id)?.projectId);
  const rows = snapshot.sessions.filter((session) => {
    const item = snapshot.items.find((candidate) => candidate.threadId === session.id);
    const matchesQuery = `${session.name} ${session.cwd}`.toLowerCase().includes(query.toLowerCase());
    const matchesProject = project === "all" || (project === NO_PROJECT_ID ? Boolean(item && !item.projectId) : item?.projectId === project);
    const matchesSource = source === "all" || session.source === source;
    const matchesState = state === "all" || (state === "archived" ? session.archived : !session.archived);
    const matchesUpdated = updatedWithin === "all" || isWithinDays(session.updatedAt, Number(updatedWithin));
    return matchesQuery && matchesProject && matchesSource && matchesState && matchesUpdated;
  }).sort(sortSessions);
  const rowItems = rows
    .map((session) => snapshot.items.find((candidate) => candidate.threadId === session.id))
    .filter((item): item is CleanupItem => Boolean(item));
  const matchingSessionIds = new Set(rows.map((session) => session.id));
  const visibleSessionIds = includeSessionAncestors(snapshot.sessions, matchingSessionIds);
  const expansionKeys = sessionTreeExpansionKeys(snapshot, visibleSessionIds);
  const allExpanded = expansionKeys.length > 0 && expansionKeys.every((key) => expanded.has(key));
  useToggleAllShortcut(rowItems, snapshot, selected, selectMany);
  return <section className="panel-card table-panel session-panel">
    <div className="filter-row session-filter-row">
      <label className="search-box"><Search size={16} /><input aria-label={t("filterSessions")} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("filterSessions")} /></label>
      <select aria-label={t("projectFilter")} value={project} onChange={(event) => setProject(event.target.value)}><option value="all">{t("allProjects")}</option>{snapshot.projects.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}{hasNoProjectSessions && <option value={NO_PROJECT_ID}>{t("noProject")}</option>}</select>
      <select aria-label={t("sourceFilter")} value={source} onChange={(event) => setSource(event.target.value)}><option value="all">{t("allSources")}</option>{[...new Set(snapshot.sessions.map((item) => item.source))].map((item) => <option key={item} value={item}>{sourceLabel(item)}</option>)}</select>
      <select aria-label={t("stateFilter")} value={state} onChange={(event) => setState(event.target.value)}><option value="all">{t("allStates")}</option><option value="active">{t("activeOnly")}</option><option value="archived">{t("archivedOnly")}</option></select>
      <select aria-label={t("updatedFilter")} value={updatedWithin} onChange={(event) => setUpdatedWithin(event.target.value)}><option value="all">{t("allTime")}</option><option value="7">{t("last7Days")}</option><option value="30">{t("last30Days")}</option></select>
    </div>
    <BulkActions items={rowItems} snapshot={snapshot} selected={selected} selectMany={selectMany} shortcut>
      <div className="view-switcher" aria-label={t("viewMode")}>
        <button type="button" className={displayMode === "tree" ? "active" : ""} aria-pressed={displayMode === "tree"} onClick={() => setDisplayMode("tree")}><GitBranch size={14} />{t("treeView")}</button>
        <button type="button" className={displayMode === "list" ? "active" : ""} aria-pressed={displayMode === "list"} onClick={() => setDisplayMode("list")}><List size={14} />{t("listView")}</button>
      </div>
      {displayMode === "tree" && expansionKeys.length > 0 && <button type="button" className="secondary-button tree-expansion-button" onClick={() => setExpanded(allExpanded ? new Set() : new Set(expansionKeys))}>{allExpanded ? <ChevronsUp size={14} /> : <ChevronsDown size={14} />}{allExpanded ? t("collapseAll") : t("expandAll")}</button>}
    </BulkActions>
    {displayMode === "tree" ? (
      <SessionTreeTable snapshot={snapshot} visibleSessionIds={visibleSessionIds} matchingSessionIds={matchingSessionIds} selected={selected} toggle={toggle} selectMany={selectMany} inspect={inspect} expanded={expanded} toggleExpanded={(key) => setExpanded((current) => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next; })} />
    ) : <SessionListTable snapshot={snapshot} rows={rows} selected={selected} toggle={toggle} selectMany={selectMany} inspect={inspect} />}
    {!rows.length && <EmptyState />}
  </section>;
}

function SessionListTable({ snapshot, rows, selected, toggle, inspect }: SelectionProps & { rows: SessionRecord[] }) {
  const { t } = useTranslation();
  return <div className="table-scroll session-list-table"><table><thead><tr><th aria-label={t("selected")} /><th>{t("name")}</th><th className="session-col-project">{t("project")}</th><th className="session-col-source">{t("source")}</th><th className="session-col-updated">{t("updated")}</th><th className="session-col-size">{t("size")}</th></tr></thead><tbody>
      {rows.map((session) => {
        const item = snapshot.items.find((candidate) => candidate.threadId === session.id)!;
        const projectName = snapshot.projects.find((candidate) => candidate.sessionIds.includes(session.id))?.name ?? t("noProject");
        return <tr key={session.id} className={`clickable-data-row ${item.blockedReason ? "row-blocked" : ""}`} tabIndex={0} aria-label={`${t("openDetails")} ${session.name}`} onClick={(event) => { if (!isInteractiveTarget(event.target)) inspect(item); }} onKeyDown={(event) => { if (event.key === "Enter" && !isInteractiveTarget(event.target)) inspect(item); }}>
          <td><CheckBox checked={selected.has(item.id)} disabled={Boolean(item.blockedReason)} onChange={() => toggle(item)} label={session.name} /></td>
          <td><div className="session-name session-detail-trigger"><span className="session-glyph"><Bot size={16} /></span><span className="session-copy"><strong title={session.name}>{session.name}</strong><span>{session.archived && <Archive size={12} />}{session.pinned && <Pin size={12} />}{session.archived ? t("archived") : statusLabel(session.status, t)}</span></span></div></td>
          <td className="session-col-project"><span className="pill" title={projectName}>{projectName}</span></td><td className="session-col-source"><span className="source-label" title={sourceLabel(session.source)}>{sourceLabel(session.source)}</span></td><td className="session-col-updated">{session.updatedAt ? relativeTime(session.updatedAt, i18n.language) : "—"}</td><td className="session-col-size"><strong>{formatBytes(session.sizeBytes)}</strong></td>
        </tr>;
      })}
    </tbody></table></div>;
}

function SessionTreeTable({ snapshot, visibleSessionIds, matchingSessionIds, selected, toggle, selectMany, inspect, expanded, toggleExpanded }: SelectionProps & { visibleSessionIds: Set<string>; matchingSessionIds: Set<string>; expanded: Set<string>; toggleExpanded: (key: string) => void }) {
  const { t } = useTranslation();
  const sessionById = new Map(snapshot.sessions.map((session) => [session.id, session]));
  const itemByThread = new Map(snapshot.items.filter((item) => item.threadId).map((item) => [item.threadId!, item]));
  const assigned = new Set(snapshot.projects.flatMap((project) => project.sessionIds));
  const noProjectIds = [...visibleSessionIds].filter((id) => !assigned.has(id));
  const projects: ProjectGroup[] = [
    ...snapshot.projects,
    ...(noProjectIds.length ? [{ id: NO_PROJECT_ID, name: t("noProject"), roots: [], sessionIds: noProjectIds, sizeBytes: noProjectIds.reduce((sum, id) => sum + (sessionById.get(id)?.sizeBytes ?? 0), 0) }] : []),
  ];

  const renderSession = (session: SessionRecord, projectIds: Set<string>, depth: number): ReactNode[] => {
    const item = itemByThread.get(session.id);
    if (!item) return [];
    const children = snapshot.sessions
      .filter((candidate) => candidate.parentThreadId === session.id && projectIds.has(candidate.id) && visibleSessionIds.has(candidate.id))
      .sort(sortSessions);
    const key = `session:${session.id}`;
    const isExpanded = expanded.has(key);
    const contextOnly = !matchingSessionIds.has(session.id);
    const rows: ReactNode[] = [<tr key={session.id} className={`clickable-data-row ${item.blockedReason ? "row-blocked" : ""} ${contextOnly ? "tree-context-row" : ""}`} tabIndex={0} aria-label={`${t("openDetails")} ${session.name}`} onClick={(event) => { if (!isInteractiveTarget(event.target)) inspect(item); }} onKeyDown={(event) => { if (event.key === "Enter" && !isInteractiveTarget(event.target)) inspect(item); }}>
        <td><CheckBox checked={selected.has(item.id)} disabled={Boolean(item.blockedReason || contextOnly)} onChange={() => toggle(item)} label={session.name} /></td>
        <td>
          <div className={`tree-session-cell ${depth > 0 ? "nested" : ""}`} style={{ "--tree-depth": depth } as CSSProperties}>
            {children.length ? <button type="button" className="tree-disclosure" aria-expanded={isExpanded} aria-label={`${isExpanded ? t("collapse") : t("expand")} ${session.name}`} onClick={() => toggleExpanded(key)}>{isExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</button> : <span className="tree-spacer" />}
            <div className="tree-session-detail">
              <span className={`session-glyph ${depth > 0 ? "child-session-glyph" : ""}`}>{depth > 0 ? <GitBranch size={15} /> : <Bot size={16} />}</span>
              <span className="session-copy"><strong title={session.name}>{session.name}</strong><span>{session.archived && <Archive size={12} />}{session.pinned && <Pin size={12} />}{contextOnly ? t("ancestorContext") : session.archived ? t("archived") : statusLabel(session.status, t)}{children.length > 0 && <> · {children.length} {t("childSessions")}</>}</span></span>
            </div>
          </div>
        </td>
        <td className="session-col-source"><span className="source-label" title={sourceLabel(session.source)}>{sourceLabel(session.source)}</span></td>
        <td className="session-col-updated">{session.updatedAt ? relativeTime(session.updatedAt, i18n.language) : "—"}</td>
        <td className="session-col-size"><strong>{formatBytes(session.sizeBytes)}</strong></td>
      </tr>];
    if (isExpanded) rows.push(...children.flatMap((child) => renderSession(child, projectIds, depth + 1)));
    return rows;
  };

  const projectBodies = projects.flatMap((project) => {
    const projectIds = new Set(project.sessionIds.filter((id) => visibleSessionIds.has(id)));
    if (!projectIds.size) return [];
    const sessions = [...projectIds].map((id) => sessionById.get(id)).filter((session): session is SessionRecord => Boolean(session));
    const roots = sessions.filter((session) => !session.parentThreadId || !projectIds.has(session.parentThreadId)).sort(sortSessions);
    const projectKey = `project:${project.id}`;
    const isExpanded = expanded.has(projectKey);
    const projectItems = sessions.map((session) => itemByThread.get(session.id)).filter((item): item is CleanupItem => Boolean(item));
    const selectableItems = projectItems.filter((item) => !item.blockedReason && matchingSessionIds.has(item.threadId!));
    const allSelected = selectableItems.length > 0 && selectableItems.every((item) => selected.has(item.id));
    const toggleProject = () => selectMany(selectableItems, !allSelected);
    const noProject = project.id === NO_PROJECT_ID;
    return [<tbody className="tree-project" key={project.id}>
      <tr className="tree-project-row">
        <td><CheckBox checked={allSelected} disabled={!selectableItems.length} onChange={toggleProject} label={noProject ? t("selectNoProjectSessions") : `${t("selectProject")} ${project.name}`} /></td>
        <td>
          <button type="button" className="project-tree-toggle" aria-expanded={isExpanded} aria-label={`${isExpanded ? t("collapse") : t("expand")} ${project.name}`} onClick={() => toggleExpanded(projectKey)}>
            {isExpanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
            <span className="project-icon compact">{noProject ? <Bot size={16} /> : <FolderCode size={16} />}</span>
            <span className="project-tree-copy"><strong title={project.name}>{project.name}</strong>{noProject ? <span>{t("sessionCount", { count: sessions.length })}</span> : <code title={project.roots[0]}>{project.roots[0]}</code>}</span>
          </button>
        </td>
        <td className="session-col-source" />
        <td className="session-col-updated" />
        <td className="session-col-size"><strong className="project-size">{formatBytes(sessions.reduce((sum, session) => sum + session.sizeBytes, 0))}</strong></td>
      </tr>
      {isExpanded && roots.flatMap((session) => renderSession(session, projectIds, 0))}
    </tbody>];
  });

  if (!projectBodies.length) return null;
  return <div className="table-scroll tree-table"><table><thead><tr><th aria-label={t("selected")} /><th>{t("hierarchy")}</th><th className="session-col-source">{t("source")}</th><th className="session-col-updated">{t("updated")}</th><th className="session-col-size">{t("size")}</th></tr></thead>{projectBodies}</table></div>;
}

function sessionTreeExpansionKeys(snapshot: InventorySnapshot, visibleSessionIds: Set<string>) {
  const assigned = new Set(snapshot.projects.flatMap((project) => project.sessionIds));
  const keys = snapshot.projects
    .filter((project) => project.sessionIds.some((id) => visibleSessionIds.has(id)))
    .map((project) => `project:${project.id}`);
  if ([...visibleSessionIds].some((id) => !assigned.has(id))) keys.push(`project:${NO_PROJECT_ID}`);
  keys.push(...snapshot.sessions
    .filter((session) => visibleSessionIds.has(session.id) && snapshot.sessions.some((candidate) => candidate.parentThreadId === session.id && visibleSessionIds.has(candidate.id)))
    .map((session) => `session:${session.id}`));
  return keys;
}

function includeSessionAncestors(sessions: SessionRecord[], matchingIds: Set<string>) {
  const byId = new Map(sessions.map((session) => [session.id, session]));
  const visible = new Set(matchingIds);
  matchingIds.forEach((id) => {
    let cursor = byId.get(id)?.parentThreadId;
    const visited = new Set<string>();
    while (cursor && !visited.has(cursor)) {
      visited.add(cursor);
      visible.add(cursor);
      cursor = byId.get(cursor)?.parentThreadId;
    }
  });
  return visible;
}

function sortSessions(left: SessionRecord, right: SessionRecord) {
  return new Date(right.updatedAt ?? 0).getTime() - new Date(left.updatedAt ?? 0).getTime();
}

function isWithinDays(value: string | undefined, days: number) {
  if (!value || !Number.isFinite(days) || days <= 0) return false;
  const timestamp = new Date(value).getTime();
  return Number.isFinite(timestamp) && timestamp >= Date.now() - days * 86_400_000;
}

function MemoryView({ snapshot, selected, toggle, selectMany, inspect }: SelectionProps) {
  const { t } = useTranslation();
  const item = snapshot.items.find((candidate) => candidate.category === "memory");
  useToggleAllShortcut(item ? [item] : [], snapshot, selected, selectMany);
  return <div className="page-stack">
    <div className="alert alert-warning memory-alert"><CircleAlert size={16} /><span>{t("memoryNotice")}</span></div>
    {item ? <section className="panel-card detail-card clickable-card" tabIndex={0} aria-label={`${t("openDetails")} ${item.title}`} onClick={(event) => { if (!isInteractiveTarget(event.target)) inspect(item); }} onKeyDown={(event) => { if (event.key === "Enter" && !isInteractiveTarget(event.target)) inspect(item); }}><div className="item-select-row"><CheckBox checked={selected.has(item.id)} disabled={!isItemSelectable(item, snapshot)} onChange={() => toggle(item)} label={item.title} /><div><h3>{item.title}</h3><p>{item.subtitle}</p></div><div className="item-select-actions"><strong>{formatBytes(item.sizeBytes)}</strong></div></div><div className="detail-lines"><span><Archive size={15} />{t("recoverable")}</span><code>{item.paths[0]}</code></div></section> : <EmptyState />}
  </div>;
}

function ItemsView({ snapshot, selected, toggle, selectMany, inspect, categories }: SelectionProps & { categories: StorageCategory[] }) {
  const { t } = useTranslation();
  const items = snapshot.items.filter((item) => categories.includes(item.category));
  useToggleAllShortcut(items, snapshot, selected, selectMany);
  if (!items.length) return <EmptyState />;
  return <div className="page-stack"><BulkActions items={items} snapshot={snapshot} selected={selected} selectMany={selectMany} shortcut /><div className="items-grid">{items.map((item) => <article className={`item-card clickable-card ${item.protected ? "protected-item" : ""}`} key={item.id} tabIndex={0} aria-label={`${t("openDetails")} ${item.title}`} onClick={(event) => { if (!isInteractiveTarget(event.target)) inspect(item); }} onKeyDown={(event) => { if (event.key === "Enter" && !isInteractiveTarget(event.target)) inspect(item); }}>
    <CheckBox checked={selected.has(item.id)} disabled={!isItemSelectable(item, snapshot)} onChange={() => toggle(item)} label={item.title} />
    <div className="item-category" style={{ color: categoryColors[item.category] }}>{categoryIcon(item.category)}</div>
    <div className="item-copy"><span>{t(categoryTranslation[item.category])}</span><h3>{item.title}</h3>{item.subtitle && <p>{item.subtitle}</p>}<code title={item.paths[0]}>{item.paths[0]}</code>{item.blockedReason && <small className="blocked-copy"><CircleAlert size={13} />{item.blockedReason}</small>}</div>
    <div className="item-size"><strong>{formatBytes(item.sizeBytes)}</strong><span className={`risk-badge ${item.risk}`}>{t(`risk${capitalize(item.risk)}`)}</span></div>
  </article>)}</div></div>;
}

function MediaView({ snapshot, selected, toggle, selectMany, inspect }: SelectionProps) {
  const items = snapshot.items.filter((item) => item.category === "attachment" || item.category === "generatedImage");
  useToggleAllShortcut(items, snapshot, selected, selectMany);
  if (!items.length) return <EmptyState icon={Image} />;
  return <div className="page-stack">
    <BulkActions items={items} snapshot={snapshot} selected={selected} selectMany={selectMany} shortcut />
    <div className="media-grid">
      {items.map((item) => <MediaCard key={item.id} item={item} snapshot={snapshot} selected={selected.has(item.id)} toggle={() => toggle(item)} inspect={() => inspect(item)} />)}
    </div>
  </div>;
}

function MediaCard({ item, snapshot, selected, toggle, inspect }: { item: CleanupItem; snapshot: InventorySnapshot; selected: boolean; toggle: () => void; inspect: () => void }) {
  const { t } = useTranslation();
  const [thumbnail, setThumbnail] = useState<string>();
  const [thumbnailState, setThumbnailState] = useState<"loading" | "ready" | "empty">("loading");
  useEffect(() => {
    let active = true;
    setThumbnail(undefined);
    setThumbnailState("loading");
    void api.getItemThumbnail(item.id)
      .then((result) => {
        if (!active) return;
        setThumbnail(result?.dataUrl);
        setThumbnailState(result ? "ready" : "empty");
      })
      .catch(() => { if (active) setThumbnailState("empty"); });
    return () => { active = false; };
  }, [item.id]);
  return <article
    className="media-card clickable-card"
    tabIndex={0}
    aria-label={`${t("openDetails")} ${item.title}`}
    onClick={(event) => { if (!isInteractiveTarget(event.target)) inspect(); }}
    onKeyDown={(event) => { if (event.key === "Enter" && !isInteractiveTarget(event.target)) inspect(); }}
  >
    <div className="media-preview" aria-busy={thumbnailState === "loading"}>
      {thumbnail ? <img src={thumbnail} alt={t("mediaPreviewAlt", { title: item.title })} /> : thumbnailState === "loading" ? <LoaderCircle size={22} className="spinning" /> : <div className="media-preview-empty" aria-label={t("previewUnavailable")}><Image size={30} /></div>}
      <div className="media-card-select" onClick={(event) => event.stopPropagation()}><CheckBox checked={selected} disabled={!isItemSelectable(item, snapshot)} onChange={toggle} label={item.title} /></div>
      <span className="media-category" style={{ color: categoryColors[item.category] }}>{t(categoryTranslation[item.category])}</span>
    </div>
    <div className="media-card-body">
      <div><h3>{item.title}</h3><strong>{formatBytes(item.sizeBytes)}</strong></div>
      {item.subtitle && <p>{item.subtitle}</p>}
      <code title={item.paths[0]}>{item.paths[0]}</code>
    </div>
  </article>;
}

function BackupsView({ backups, restore, requestPurge, busy }: { backups: BackupRecord[]; restore: (id: string) => void; requestPurge: (backup: BackupRecord) => void; busy: boolean }) {
  const { t } = useTranslation();
  return <div className="page-stack">{!backups.length ? <EmptyState icon={Archive} label={t("noBackups")} /> : <div className="backup-list">{backups.map((backup) => <article className="backup-card" key={backup.id}><div className="backup-icon"><Archive size={19} /></div><div><strong>{new Date(backup.createdAt).toLocaleString()}</strong><span>{backup.itemCount} {t("items")} · {formatBytes(backup.originalBytes)} · {t("archiveSize")} {formatBytes(backup.archiveBytes)}</span><code>{backup.id}</code></div><div className="backup-expiry"><span>{t("expires")}</span><strong>{new Date(backup.expiresAt).toLocaleDateString()}</strong></div><button className="secondary-button" disabled={busy} onClick={() => restore(backup.id)}><RotateCcw size={15} />{t("restore")}</button><button className="secondary-button danger" disabled={busy} onClick={() => requestPurge(backup)}><Trash2 size={15} />{t("deleteForever")}</button></article>)}</div>}</div>;
}

function SettingsView({ value, onSave }: { value: AppSettings; onSave: (settings: AppSettings) => Promise<void> }) {
  const { t } = useTranslation();
  const [form, setForm] = useState(value);
  const [saving, setSaving] = useState(false);
  const persistedPreferences = useRef(value);
  useEffect(() => {
    persistedPreferences.current = value;
    setForm(value);
  }, [value]);
  useEffect(() => () => { void applyPreferences(persistedPreferences.current); }, []);
  useEffect(() => watchSystemPreferences(form), [form.locale, form.theme]);
  const preview = (next: AppSettings) => {
    setForm(next);
    void applyPreferences(next);
  };
  return <form className="settings-form" onSubmit={(event) => { event.preventDefault(); setSaving(true); void onSave(form).finally(() => setSaving(false)); }}>
    <section className="settings-group">
      <h3 className="settings-group-label">{t("settingsGeneral")}</h3>
      <Setting label={t("codexHome")} hint={t("codexHomeHint")}><input aria-label={t("codexHome")} value={form.customCodexHome ?? ""} onChange={(event) => setForm({ ...form, customCodexHome: event.target.value || undefined })} placeholder="~/.codex" /></Setting>
      <Setting label={t("language")}><div className="segmented" role="group" aria-label={t("language")}><button type="button" className={form.locale === "system" ? "active" : ""} aria-pressed={form.locale === "system"} onClick={() => preview({ ...form, locale: "system" })}>{t("system")}</button><button type="button" className={form.locale === "zh" ? "active" : ""} aria-pressed={form.locale === "zh"} onClick={() => preview({ ...form, locale: "zh" })}>{t("chinese")}</button><button type="button" className={form.locale === "en" ? "active" : ""} aria-pressed={form.locale === "en"} onClick={() => preview({ ...form, locale: "en" })}>{t("english")}</button></div></Setting>
      <Setting label={t("appearance")}><div className="segmented" role="group" aria-label={t("appearance")}><button type="button" className={form.theme === "system" ? "active" : ""} aria-pressed={form.theme === "system"} onClick={() => preview({ ...form, theme: "system" })}><Monitor size={14} />{t("system")}</button><button type="button" className={form.theme === "light" ? "active" : ""} aria-pressed={form.theme === "light"} onClick={() => preview({ ...form, theme: "light" })}><Sun size={14} />{t("light")}</button><button type="button" className={form.theme === "dark" ? "active" : ""} aria-pressed={form.theme === "dark"} onClick={() => preview({ ...form, theme: "dark" })}><Moon size={14} />{t("dark")}</button></div></Setting>
    </section>
    <section className="settings-group">
      <h3 className="settings-group-label">{t("settingsRetention")}</h3>
      <div className="retention-grid"><Setting label={t("backupRetention")}><input aria-label={t("backupRetention")} type="number" min="1" max="3650" value={form.backupRetentionDays} onChange={(event) => setForm({ ...form, backupRetentionDays: Number(event.target.value) })} /></Setting><Setting label={t("logRetention")}><input aria-label={t("logRetention")} type="number" min="1" max="365" value={form.logRetentionDays} onChange={(event) => setForm({ ...form, logRetentionDays: Number(event.target.value) })} /></Setting><Setting label={t("tempRetention")}><input aria-label={t("tempRetention")} type="number" min="1" max="8760" value={form.tempRetentionHours} onChange={(event) => setForm({ ...form, tempRetentionHours: Number(event.target.value) })} /></Setting></div>
    </section>
    <div className="settings-footer"><button className="primary-button" type="submit" disabled={saving}>{saving && <LoaderCircle size={14} className="spinning" />}{saving ? t("savingSettings") : t("save")}</button></div>
  </form>;
}

function Setting({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return <div className="setting-row"><div><strong>{label}</strong>{hint && <span>{hint}</span>}</div>{children}</div>;
}

function ItemDetailDialog({ item, snapshot, selected, toggle, close }: { item: CleanupItem; snapshot: InventorySnapshot; selected: boolean; toggle: () => void; close: () => void }) {
  const { t } = useTranslation();
  const [content, setContent] = useState<ItemContentDetail>();
  const [contentError, setContentError] = useState<string>();
  const [contentLoading, setContentLoading] = useState(true);
  const session = item.threadId ? snapshot.sessions.find((candidate) => candidate.id === item.threadId) : undefined;
  const project = item.projectId ? snapshot.projects.find((candidate) => candidate.id === item.projectId) : undefined;
  const selectable = isItemSelectable(item, snapshot);
  const metadata = Object.entries(item.metadata).filter(([key]) => !["source", "status", "pinned"].includes(key));
  useEffect(() => {
    let active = true;
    setContent(undefined);
    setContentError(undefined);
    setContentLoading(true);
    void api.getItemContent(item.id)
      .then((result) => { if (active) setContent(result); })
      .catch((reason) => { if (active) setContentError(messageOf(reason)); })
      .finally(() => { if (active) setContentLoading(false); });
    return () => { active = false; };
  }, [item.id]);
  return <div className="detail-drawer-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) close(); }}>
    <aside className="item-detail-dialog" role="dialog" aria-modal="true" aria-labelledby="item-detail-title">
      <header className="item-detail-header">
        <div className="item-detail-category" style={{ color: categoryColors[item.category] }}>{categoryIcon(item.category)}</div>
        <div><span>{t(categoryTranslation[item.category])}</span><h2 id="item-detail-title">{item.title}</h2></div>
        <button type="button" className="icon-button" onClick={close} aria-label={t("closeDetails")}><X size={18} /></button>
      </header>
      <div className="item-detail-scroll">
        {item.subtitle && <p className="item-detail-lead">{item.subtitle}</p>}
        <div className="detail-summary-grid">
          <DetailFact label={t("size")} value={formatBytes(item.sizeBytes)} />
          <DetailFact label={t("detailsRisk")} value={t(`risk${capitalize(item.risk)}`)} tone={item.risk} />
          <DetailFact label={t("detailsRecovery")} value={item.recoverable ? t("yes") : t("no")} />
        </div>

        <section className="detail-section">
          <h3>{t("detailsItem")}</h3>
          <dl className="detail-facts">
            <div><dt>{t("detailsIdentifier")}</dt><dd><code>{item.id}</code></dd></div>
            <div><dt>{t("detailsModified")}</dt><dd>{item.modifiedAt ? formatDate(item.modifiedAt, i18n.language) : "—"}</dd></div>
            <div><dt>{t("detailsSelection")}</dt><dd>{selected ? t("selectedState") : selectable ? t("notSelectedState") : t("unavailableState")}</dd></div>
          </dl>
        </section>

        {session && <section className="detail-section">
          <h3>{t("detailsSession")}</h3>
          <dl className="detail-facts">
            <div><dt>{t("detailsSessionId")}</dt><dd><code>{session.id}</code></dd></div>
            <div><dt>{t("project")}</dt><dd>{project?.name ?? t("noProject")}</dd></div>
            <div><dt>{t("source")}</dt><dd>{sourceLabel(session.source)}</dd></div>
            <div><dt>{t("detailsStatus")}</dt><dd>{session.archived ? t("archived") : statusLabel(session.status, t)}{session.pinned ? ` · ${t("pinned")}` : ""}</dd></div>
            <div><dt>{t("detailsCreated")}</dt><dd>{session.createdAt ? formatDate(session.createdAt, i18n.language) : "—"}</dd></div>
            <div><dt>{t("updated")}</dt><dd>{session.updatedAt ? formatDate(session.updatedAt, i18n.language) : "—"}</dd></div>
            <div><dt>{t("detailsParent")}</dt><dd><code>{session.parentThreadId ?? "—"}</code></dd></div>
            <div><dt>{t("detailsChildren")}</dt><dd>{session.descendantIds.length}</dd></div>
            <div className="detail-fact-wide"><dt>{t("detailsWorkingDirectory")}</dt><dd>{session.cwd ? <code>{session.cwd}</code> : t("noWorkingDirectory")}</dd></div>
            {project?.roots[0] && <div className="detail-fact-wide"><dt>{t("detailsProjectRoot")}</dt><dd><code>{project.roots[0]}</code></dd></div>}
          </dl>
        </section>}

        <section className="detail-section content-preview-section" aria-busy={contentLoading}>
          <div className="content-preview-heading">
            <h3>{t("detailsContent")}</h3>
            {content && <span>{t("contentSource")}: {contentSourceLabel(content.source, t)}</span>}
          </div>
          {contentLoading ? <ContentPreviewSkeleton /> : contentError ? (
            <div className="detail-blocker"><CircleAlert size={17} /><div><strong>{t("contentLoadFailed")}</strong><span>{contentError}</span></div></div>
          ) : content ? <ContentPreview detail={content} /> : null}
        </section>

        <section className="detail-section">
          <h3>{t("detailsPaths")}</h3>
          {item.paths.length ? <div className="detail-paths">{item.paths.map((path) => <code key={path}>{path}</code>)}</div> : <p className="detail-empty">{t("noPaths")}</p>}
        </section>

        {metadata.length > 0 && <section className="detail-section">
          <h3>{t("detailsMetadata")}</h3>
          <dl className="detail-facts">{metadata.map(([key, value]) => <div key={key}><dt>{metadataLabel(key, t)}</dt><dd>{metadataValue(key, value, t)}</dd></div>)}</dl>
        </section>}

        {item.blockedReason && <div className="detail-blocker"><CircleAlert size={17} /><div><strong>{t("blocked")}</strong><span>{item.blockedReason}</span></div></div>}
      </div>
      <footer className="item-detail-footer">
        <button type="button" className="secondary-button" onClick={close}>{t("close")}</button>
        {selectable && <button type="button" className={selected ? "secondary-button" : "primary-button"} onClick={toggle}>{selected ? t("deselectItem") : t("selectItem")}</button>}
      </footer>
    </aside>
  </div>;
}

function DetailFact({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return <div><span>{label}</span><strong className={tone ? `detail-tone ${tone}` : undefined}>{value}</strong></div>;
}

function ContentPreview({ detail }: { detail: ItemContentDetail }) {
  const { t } = useTranslation();
  return <div className="content-preview">
    {detail.warning && <div className="content-warning"><CircleAlert size={15} /><span>{detail.warning}</span></div>}
    {detail.blocks.map((block, index) => <ContentBlockView block={block} key={`${block.kind}:${index}`} />)}
    {detail.truncated && <div className="content-limit"><span>{t("contentTruncated")}</span><strong>{formatBytes(detail.bytesRead)}</strong></div>}
  </div>;
}

function ContentBlockView({ block }: { block: ContentBlock }) {
  const { t } = useTranslation();
  if (block.kind === "message") return <article className={`content-message role-${block.role}`}>
    <header><strong>{roleLabel(block.role, t)}</strong>{block.phase && <span>{block.phase.replace("final_answer", "final")}</span>}</header>
    <div className="rendered-text">{block.text}</div>
  </article>;
  if (block.kind === "image") return <figure className="content-image"><img src={block.dataUrl} alt={block.title} /><figcaption>{block.title}</figcaption></figure>;
  if (block.kind === "log") return <article className="content-log">
    <header>{block.level && <strong className={`log-${block.level.toLowerCase()}`}>{block.level}</strong>}<span>{block.target}</span><time>{block.timestamp ? formatDate(block.timestamp, i18n.language) : ""}</time></header>
    <div className="rendered-text mono-text">{block.text}</div>
  </article>;
  if (block.kind === "notice") return <div className="content-notice">{block.text}</div>;
  return <article className="content-text-block"><strong>{block.title}</strong><div className="rendered-text mono-text">{block.text}</div></article>;
}

function ContentPreviewSkeleton() {
  const { t } = useTranslation();
  return <div className="content-preview-skeleton" aria-label={t("loadingContent")}>
    <div className="skeleton skeleton-line skeleton-short" />
    <div className="skeleton skeleton-line" />
    <div className="skeleton skeleton-line skeleton-medium" />
  </div>;
}

function ReviewDialog({ plan, snapshot, createBackup, setCreateBackup, close, execute, executing }: { plan: CleanupPlan; snapshot: InventorySnapshot; createBackup: boolean; setCreateBackup: (value: boolean) => void; close: () => void; execute: () => void; executing: boolean }) {
  const { t } = useTranslation();
  const selected = snapshot.items.filter((item) => plan.selectedItemIds.includes(item.id));
  const descendantCount = Math.max(0, plan.expandedSessionIds.length - selected.filter((item) => item.threadId).length);
  const canBackup = plan.estimatedBackupBytes > 0;
  const displayedBackupBytes = createBackup ? plan.estimatedBackupBytes : 0;
  const withoutBackup = canBackup && !createBackup;
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !executing) close(); }}><section className="review-dialog" role="dialog" aria-modal="true" aria-labelledby="review-title">
    <button className="icon-button modal-close" onClick={close} disabled={executing}><X size={18} /></button>
    <h2 id="review-title">{t("reviewTitle")}</h2>
    <div className="review-metrics"><div><span>{t("affected")}</span><strong>{selected.length}</strong></div><div><span>{t("descendants")}</span><strong>{descendantCount}</strong></div><div><span>{t("backupSize")}</span><strong>{formatBytes(displayedBackupBytes)}</strong></div><div><span>{t("netGain")}</span><strong>{formatBytes(Math.max(0, plan.estimatedBytes - displayedBackupBytes * 0.62))}</strong></div></div>
    <div className="impact-list">{selected.slice(0, 6).map((item) => <div key={item.id}><span style={{ color: categoryColors[item.category] }}>{categoryIcon(item.category)}</span><div><strong>{item.title}</strong><span>{t(categoryTranslation[item.category])}</span></div><strong>{formatBytes(item.sizeBytes)}</strong></div>)}</div>
    {plan.blockers.length > 0 && <div className="blocker-box"><CircleAlert size={18} /><div><strong>{t("blockers")}</strong>{plan.blockers.map((blocker) => <span key={blocker}>{blocker}</span>)}</div></div>}
    {canBackup && <label className="backup-option"><input type="checkbox" checked={createBackup} disabled={executing} onChange={(event) => setCreateBackup(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("createBackupOption")}</strong></label>}
    {withoutBackup && <div className="no-backup-warning"><CircleAlert size={16} /><span>{t("noBackupWarning")}</span></div>}
    <div className="modal-actions"><button className="secondary-button" onClick={close} disabled={executing}>{t("cancel")}</button><button className="primary-button danger-primary" onClick={execute} disabled={plan.blockers.length > 0 || executing}>{executing && <LoaderCircle size={16} className="spinning" />}{executing ? t("executing") : createBackup && canBackup ? t("backupAndExecute") : t("executeWithoutBackup")}</button></div>
  </section></div>;
}

function PurgeBackupDialog({ backup, close, purge, purging }: { backup: BackupRecord; close: () => void; purge: () => void; purging: boolean }) {
  const { t } = useTranslation();
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !purging) close(); }}>
    <section className="review-dialog purge-dialog" role="dialog" aria-modal="true" aria-labelledby="purge-backup-title">
      <button className="icon-button modal-close" onClick={close} disabled={purging} aria-label={t("cancel")}><X size={18} /></button>
      <div className="destructive-dialog-mark"><Trash2 size={20} /></div>
      <h2 id="purge-backup-title">{t("deleteBackupTitle")}</h2>
      <div className="purge-backup-summary">
        <div><span>{t("created")}</span><strong>{new Date(backup.createdAt).toLocaleString()}</strong></div>
        <div><span>{t("archiveSize")}</span><strong>{formatBytes(backup.archiveBytes)}</strong></div>
        <code>{backup.archivePath}</code>
      </div>
      <div className="no-backup-warning"><CircleAlert size={16} /><span>{t("deleteBackupWarning")}</span></div>
      <div className="modal-actions">
        <button className="secondary-button" onClick={close} disabled={purging}>{t("cancel")}</button>
        <button className="primary-button danger-primary" onClick={purge} disabled={purging}>{purging && <LoaderCircle size={16} className="spinning" />}{purging ? t("deletingBackup") : t("confirmDeleteBackup")}</button>
      </div>
    </section>
  </div>;
}

function BulkActions({ items, snapshot, selected, selectMany, shortcut = false, children }: { items: CleanupItem[]; snapshot: InventorySnapshot; selected: Set<string>; selectMany: (items: CleanupItem[], shouldSelect: boolean) => void; shortcut?: boolean; children?: ReactNode }) {
  const { t } = useTranslation();
  const selectable = items.filter((item) => isItemSelectable(item, snapshot));
  const selectedCount = selectable.filter((item) => selected.has(item.id)).length;
  const allSelected = selectable.length > 0 && selectedCount === selectable.length;
  return <div className="bulk-actions" role="toolbar" aria-label={t("bulkSelection")}>
    <button type="button" className="secondary-button bulk-select-button" disabled={!selectable.length} onClick={() => selectMany(selectable, !allSelected)}>{allSelected ? <X size={14} /> : <Check size={14} />}{allSelected ? t("deselectAllResults") : t("selectAllResults")}</button>
    <span>{t("selectionScope", { selected: selectedCount, total: selectable.length })}{shortcut && <kbd>⌘/Ctrl A</kbd>}</span>
    {children}
  </div>;
}

function CheckBox({ checked, disabled, onChange, label }: { checked: boolean; disabled?: boolean; onChange: () => void; label: string }) {
  return <label className={`checkbox ${disabled ? "disabled" : ""}`} title={disabled ? label : undefined}><input type="checkbox" checked={checked} disabled={disabled} onChange={onChange} aria-label={label} /><span><Check size={12} /></span></label>;
}

function StorageLoadingView({ view, failed }: { view: ViewId; failed: boolean }) {
  const { t } = useTranslation();
  const status = <div className={`scan-placeholder-status ${failed ? "scan-placeholder-failed" : ""}`} role="status">
    {failed ? <CircleAlert size={16} /> : <LoaderCircle size={16} className="spinning" />}
    <span>{failed ? t("scanFailed") : t("waitingForScanData")}</span>
  </div>;
  if (view === "overview") return <div className="page-stack storage-skeleton" aria-busy={!failed}>
    {status}
    <section className="stat-strip">{[0, 1, 2].map((index) => <div className="stat" key={index}><span className="skeleton skeleton-line skeleton-short" /><span className="skeleton skeleton-value" /><span className="skeleton skeleton-line skeleton-medium" /></div>)}</section>
    <section className="panel-card skeleton-panel"><div className="panel-heading"><h3>{t("storageBreakdown")}</h3><span>{t("waitingForScanData")}</span></div><div className="overview-chart-layout"><div className="storage-donut-figure skeleton-donut-figure"><span className="skeleton-donut" /><span className="skeleton skeleton-line skeleton-medium" /></div><div className="category-list">{[0, 1, 2, 3].map((index) => <div className="category-row" key={index}><span className="skeleton skeleton-dot" /><div><span className="skeleton skeleton-line skeleton-medium" /><span className="skeleton skeleton-line skeleton-short" /></div><div className="skeleton skeleton-track" /><span className="skeleton skeleton-line" /></div>)}</div></div></section>
  </div>;
  if (view === "sessions") return <div className="page-stack storage-skeleton" aria-busy={!failed}>
    {status}
    <section className="panel-card table-panel"><div className="filter-row session-filter-row"><div className="search-box skeleton-control"><Search size={16} /><span>{t("filterSessions")}</span></div>{[0, 1, 2, 3].map((index) => <div className="skeleton-control compact" key={index}><span className="skeleton skeleton-line" /></div>)}</div><div className="bulk-actions"><span>{t("waitingForScanData")}</span></div><div className="skeleton-table">{[0, 1, 2, 3, 4].map((index) => <div key={index}><span className="skeleton skeleton-dot" /><span className="skeleton skeleton-line" /><span className="skeleton skeleton-line skeleton-short" /><span className="skeleton skeleton-line skeleton-short" /></div>)}</div></section>
  </div>;
  if (view === "memory") return <div className="page-stack storage-skeleton" aria-busy={!failed}>
    {status}
    <section className="panel-card detail-card skeleton-detail-card"><span className="skeleton skeleton-dot" /><div><span className="skeleton skeleton-value" /><span className="skeleton skeleton-line skeleton-medium" /></div><span className="skeleton skeleton-line skeleton-short" /></section>
  </div>;
  return <div className="page-stack storage-skeleton" aria-busy={!failed}>
    {status}
    <div className="bulk-actions"><span>{t("waitingForScanData")}</span></div>
    <div className="items-grid">{[0, 1, 2].map((index) => <article className="item-card" key={index}><span className="skeleton skeleton-dot" /><span className="skeleton skeleton-icon" /><div className="skeleton-copy"><span className="skeleton skeleton-line skeleton-short" /><span className="skeleton skeleton-value" /><span className="skeleton skeleton-line" /></div><span className="skeleton skeleton-line skeleton-short" /></article>)}</div>
  </div>;
}

function LoadingState({ label, icon: Icon }: { label: string; icon?: typeof Settings }) { return <div className="loading-state">{Icon ? <Icon /> : <LoaderCircle className="spinning" />}<strong>{label}</strong></div>; }
function EmptyState({ icon: Icon = Boxes, label }: { icon?: typeof Boxes; label?: string }) { const { t } = useTranslation(); return <div className="empty-state"><Icon size={28} /><strong>{label ?? t("noItems")}</strong></div>; }

function categoryIcon(category: StorageCategory) {
  const props = { size: 18 };
  if (category === "memory") return <MemoryStick {...props} />;
  if (category === "attachment" || category === "generatedImage") return <Image {...props} />;
  if (category === "session") return <Bot {...props} />;
  if (category === "archivedSession") return <Archive {...props} />;
  if (category === "protected") return <LockKeyhole {...props} />;
  return <Database {...props} />;
}

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let amount = value / 1024;
  let unit = units[0];
  for (let index = 1; amount >= 1024 && index < units.length; index += 1) { amount /= 1024; unit = units[index]; }
  return `${amount >= 10 ? amount.toFixed(0) : amount.toFixed(1)} ${unit}`;
}

function formatDate(value: string, language: string) {
  return new Intl.DateTimeFormat(language.startsWith("zh") ? "zh-CN" : "en", { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
}

function capitalize(value: string) { return `${value.charAt(0).toUpperCase()}${value.slice(1)}`; }

function isItemSelectable(item: CleanupItem, snapshot: InventorySnapshot) {
  if (item.protected || item.blockedReason) return false;
  return item.category !== "memory" || snapshot.installation.capabilities.memoryReset;
}

function isInteractiveTarget(target: EventTarget | null) {
  return target instanceof Element && Boolean(target.closest("button, input, label, select, textarea, a, [contenteditable='true']"));
}

function useToggleAllShortcut(items: CleanupItem[], snapshot: InventorySnapshot, selected: Set<string>, selectMany: (items: CleanupItem[], shouldSelect: boolean) => void) {
  const selectable = items.filter((item) => isItemSelectable(item, snapshot));
  const allSelected = selectable.length > 0 && selectable.every((item) => selected.has(item.id));
  useEffect(() => {
    const toggleFiltered = (event: KeyboardEvent) => {
      const target = event.target;
      const isEditing = target instanceof HTMLElement && Boolean(target.closest("input, textarea, select, [contenteditable='true']"));
      const modalOpen = Boolean(document.querySelector("[aria-modal='true']"));
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a" && !event.repeat && !isEditing && !modalOpen && selectable.length > 0) {
        event.preventDefault();
        selectMany(selectable, !allSelected);
      }
    };
    window.addEventListener("keydown", toggleFiltered);
    return () => window.removeEventListener("keydown", toggleFiltered);
  }, [allSelected, selectMany, selectable]);
}

function metadataLabel(key: string, t: (key: string) => string) {
  const labels: Record<string, string> = {
    retentionDays: "detailsRetentionDays",
    olderThanHours: "detailsOlderThanHours",
    requiresCodexExit: "detailsRequiresCodexExit",
    regenerable: "detailsRegenerable",
    scope: "detailsScope",
    files: "detailsFiles",
    entries: "detailsEntries",
    association: "detailsAssociation",
    entryType: "detailsEntryType",
    protection: "detailsProtection",
  };
  if (labels[key]) return t(labels[key]);
  return key.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (character) => character.toUpperCase());
}

function metadataValue(key: string, value: string, t: (key: string) => string) {
  if (["regenerable", "requiresCodexExit"].includes(key)) return value === "true" ? t("yes") : t("no");
  if (key === "scope" && value === "global") return t("globalScope");
  if (key === "association" && value === "orphaned") return t("orphanedAssociation");
  if (key === "entryType" && value === "directory") return t("directoryEntry");
  if (key === "entryType" && value === "file") return t("fileEntry");
  if (key === "protection" && value === "always") return t("alwaysProtected");
  return value;
}

function relativeTime(value: string, language: string) {
  const seconds = Math.round((new Date(value).getTime() - Date.now()) / 1000);
  const formatter = new Intl.RelativeTimeFormat(language.startsWith("zh") ? "zh-CN" : "en", { numeric: "auto" });
  if (Math.abs(seconds) < 60) return formatter.format(seconds, "second");
  if (Math.abs(seconds) < 3600) return formatter.format(Math.round(seconds / 60), "minute");
  if (Math.abs(seconds) < 86_400) return formatter.format(Math.round(seconds / 3600), "hour");
  return formatter.format(Math.round(seconds / 86_400), "day");
}

function statusLabel(status: string, t: (key: string) => string) { const normalized = status.toLowerCase(); return normalized === "active" || normalized === "loaded" ? t("active") : t("notLoaded"); }
function sourceLabel(source: string) { return source === "vscode" ? "Desktop / IDE" : source; }
function roleLabel(role: string, t: (key: string) => string) {
  if (role === "user") return t("contentRoleUser");
  if (role === "assistant") return t("contentRoleAssistant");
  if (role === "system") return t("contentRoleSystem");
  if (role === "tool") return t("contentRoleTool");
  return role;
}
function contentSourceLabel(source: string, t: (key: string) => string) {
  if (source === "appServer.thread/read") return t("contentSourceAppServer");
  if (source === "rollout.readOnlyFallback") return t("contentSourceRollout");
  if (source === "recognizedMemoryDb.readOnly" || source === "recognizedLogDb.readOnly") return t("contentSourceDatabase");
  if (source === "filesystem.readOnly") return t("contentSourceFilesystem");
  return source;
}
function messageOf(reason: unknown) { return reason instanceof Error ? reason.message : String(reason); }
