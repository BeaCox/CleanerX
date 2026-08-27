import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  AgentInstallation,
  AgentKind,
  BackupRecord,
  CleanupItem,
  CleanupPlan,
  CleanupResult,
  InventorySnapshot,
  ItemContentDetail,
  ItemThumbnail,
  SessionRecord,
  StorageCategory,
} from "./types";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const inTauri = () => Boolean(window.__TAURI_INTERNALS__);
let mockSnapshot = createMockSnapshot();
const mockPlans = new Map<string, CleanupPlan>();
const defaultMockSettings: AppSettings = {
  activeAgent: "codex",
  locale: "system",
  theme: "system",
  backupRetentionDays: 30,
  logRetentionDays: 7,
  tempRetentionHours: 24,
};
let mockBackups: BackupRecord[] = [
  {
    id: "7f9fd849-9817-46ef-b075-7a437d32b03c",
    createdAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
    expiresAt: new Date(Date.now() + 28 * 86_400_000).toISOString(),
    archivePath: "/Users/demo/Library/Application Support/CleanerX/backups/7f9fd849-9817-46ef-b075-7a437d32b03c.cxb",
    archiveBytes: 8_420_000,
    originalBytes: 15_760_000,
    itemCount: 1,
    operationId: "2f6d7c93-2ba1-40d0-a1dd-c6165915389d",
    agent: "codex",
  },
];

const mockPreviewDataUrl = (accent: string, label: string) => `data:image/svg+xml;charset=UTF-8,${encodeURIComponent(`
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 400">
    <rect width="640" height="400" fill="#eee9dd"/>
    <rect x="48" y="46" width="544" height="308" fill="#faf8f2" stroke="#bcb4a1" stroke-width="2"/>
    <rect x="78" y="82" width="210" height="20" fill="${accent}"/>
    <rect x="78" y="126" width="484" height="10" fill="#d9d3c4"/>
    <rect x="78" y="152" width="390" height="10" fill="#d9d3c4"/>
    <circle cx="458" cy="258" r="58" fill="none" stroke="${accent}" stroke-width="22"/>
    <text x="78" y="310" fill="#6d6759" font-family="system-ui" font-size="24">${label}</text>
  </svg>
`)}`;

export const api = {
  async detectAgents(): Promise<AgentInstallation[]> {
    if (inTauri()) return invoke("detect_agents");
    return [
      createMockSnapshot("codex").installation,
      createMockSnapshot("claudeCode").installation,
    ];
  },

  async scanStorage(targetAgent?: AgentKind): Promise<InventorySnapshot> {
    if (inTauri()) return invoke("scan_storage", { targetAgent });
    await delay(420);
    if (targetAgent && mockSnapshot.installation.kind !== targetAgent) {
      mockSnapshot = createMockSnapshot(targetAgent);
    }
    mockSnapshot = { ...mockSnapshot, scannedAt: new Date().toISOString() };
    return structuredClone(mockSnapshot);
  },

  async planCleanup(selectedItemIds: string[]): Promise<CleanupPlan> {
    if (inTauri()) return invoke("plan_cleanup", { selectedItemIds });
    const selected = mockSnapshot.items.filter((item) => selectedItemIds.includes(item.id));
    const sessions = selected.filter((item) => item.threadId);
    const expanded = new Set<string>();
    sessions.forEach((item) => {
      if (!item.threadId) return;
      expanded.add(item.threadId);
      mockSnapshot.sessions
        .find((session) => session.id === item.threadId)
        ?.descendantIds.forEach((id) => expanded.add(id));
    });
    const plan = {
      id: crypto.randomUUID(),
      snapshotId: mockSnapshot.id,
      createdAt: new Date().toISOString(),
      selectedItemIds,
      expandedSessionIds: [...expanded],
      operations: [],
      estimatedBytes: selected.reduce((total, item) => total + item.sizeBytes, 0),
      estimatedBackupBytes: selected
        .filter((item) => item.recoverable)
        .reduce((total, item) => total + item.sizeBytes, 0),
      blockers: selected.flatMap((item) => item.blockedReason ?? []),
    };
    mockPlans.set(plan.id, plan);
    return plan;
  },

  async getItemContent(itemId: string): Promise<ItemContentDetail> {
    if (inTauri()) return invoke("get_item_content", { itemId });
    await delay(180);
    const item = mockSnapshot.items.find((candidate) => candidate.id === itemId);
    if (!item) throw new Error("Item is not part of the current scan");
    if (item.threadId) {
      return {
        itemId,
        source: mockSnapshot.installation.kind === "codex" ? "appServer.thread/read" : "claudeTranscript.readOnly",
        truncated: false,
        bytesRead: 236,
        blocks: [
          { kind: "message", role: "user", text: "把项目与会话整理为树状视图，并保留项目根目录。" },
          { kind: "message", role: "assistant", phase: "commentary", text: "我先梳理现有层级和筛选行为，再调整会话视图。" },
          { kind: "text", title: "Command", text: "$ cargo test --workspace\n\nall tests passed" },
          { kind: "message", role: "assistant", phase: "final_answer", text: "会话树已经按项目根目录聚合，子会话会显示在父会话下。" },
        ],
      };
    }
    if (item.category === "memory") {
      return {
        itemId,
        source: mockSnapshot.installation.kind === "codex" ? "recognizedMemoryDb.readOnly" : "claudeMemoryMarkdown.readOnly",
        truncated: false,
        bytesRead: 152,
        blocks: [
          { kind: "text", title: "CleanerX · 2026-08-25", text: "用户偏好简洁的桌面界面；项目根目录用于聚合会话，不应删除源码目录。" },
        ],
      };
    }
    if (item.category === "log") {
      return {
        itemId,
        source: "recognizedLogDb.readOnly",
        truncated: false,
        bytesRead: 96,
        blocks: [
          { kind: "log", timestamp: new Date().toISOString(), level: "INFO", target: "codex_app_server", text: "thread/list completed · 5 rows" },
          { kind: "log", timestamp: new Date(Date.now() - 120_000).toISOString(), level: "WARN", target: "codex_storage", text: "stale cache entry ignored" },
        ],
      };
    }
    if (item.category === "generatedImage") {
      const dataUrl = mockPreviewDataUrl("#8f6fc0", item.title);
      return {
        itemId,
        source: "filesystem.readOnly",
        truncated: false,
        bytesRead: dataUrl.length,
        blocks: [{ kind: "image", title: item.title, dataUrl }],
      };
    }
    return {
      itemId,
      source: "filesystem.readOnly",
      truncated: false,
      bytesRead: 72,
      blocks: [{ kind: "text", title: item.paths[0] ?? item.title, text: "Preview of the selected local item." }],
    };
  },

  async getItemThumbnail(itemId: string): Promise<ItemThumbnail | undefined> {
    if (inTauri()) return invoke("get_item_thumbnail", { itemId });
    await delay(90);
    const item = mockSnapshot.items.find((candidate) => candidate.id === itemId);
    if (!item || (item.category !== "attachment" && item.category !== "generatedImage")) return undefined;
    if (item.id === "attachment:1") return undefined;
    return {
      itemId,
      title: item.title,
      dataUrl: mockPreviewDataUrl(
        item.category === "attachment" ? "#b08344" : "#8f6fc0",
        item.title,
      ),
    };
  },

  async executeCleanup(planId: string, createBackup: boolean): Promise<CleanupResult> {
    if (inTauri()) return invoke("execute_cleanup", { planId, createBackup });
    await delay(850);
    const plan = mockPlans.get(planId);
    const backupId = createBackup ? crypto.randomUUID() : undefined;
    if (backupId && plan) {
      mockBackups = [{
        id: backupId,
        createdAt: new Date().toISOString(),
        expiresAt: new Date(Date.now() + readMockSettings().backupRetentionDays * 86_400_000).toISOString(),
        archivePath: `/Users/demo/Library/Application Support/CleanerX/backups/${backupId}.cxb`,
        archiveBytes: Math.round(plan.estimatedBackupBytes * 0.62),
        originalBytes: plan.estimatedBackupBytes,
        itemCount: plan.selectedItemIds.length,
        operationId: plan.id,
        agent: mockSnapshot.installation.kind,
      }, ...mockBackups];
    }
    return {
      operationId: planId,
      status: "complete",
      backupId,
      reclaimedBytes: plan?.estimatedBytes ?? 0,
      deletedItemIds: [],
      warnings: [],
    };
  },

  async listBackups(): Promise<BackupRecord[]> {
    if (inTauri()) return invoke("list_backups");
    return structuredClone(mockBackups);
  },

  async restoreBackup(backupId: string): Promise<void> {
    if (inTauri()) await invoke("restore_backup", { backupId });
  },

  async purgeBackup(backupId: string): Promise<void> {
    if (inTauri()) {
      await invoke("purge_backup", { backupId });
      return;
    }
    await delay(180);
    if (!mockBackups.some((backup) => backup.id === backupId)) throw new Error("Backup not found");
    mockBackups = mockBackups.filter((backup) => backup.id !== backupId);
  },

  async getSettings(): Promise<AppSettings> {
    if (inTauri()) return invoke("get_settings");
    return readMockSettings();
  },

  async updateSettings(settings: AppSettings): Promise<AppSettings> {
    if (inTauri()) return invoke("update_settings", { settings });
    window.localStorage.setItem("cleanerx.mock.settings.v1", JSON.stringify(settings));
    return structuredClone(settings);
  },
};

const delay = (milliseconds: number) =>
  new Promise((resolve) => window.setTimeout(resolve, milliseconds));

function readMockSettings(): AppSettings {
  try {
    const stored = window.localStorage.getItem("cleanerx.mock.settings.v1");
    return stored
      ? { ...defaultMockSettings, ...JSON.parse(stored) as AppSettings }
      : structuredClone(defaultMockSettings);
  } catch {
    return structuredClone(defaultMockSettings);
  }
}

function createMockSnapshot(kind: AgentKind = "codex"): InventorySnapshot {
  const sessions: SessionRecord[] = [
    session("019f…c47", "CleanerX", "", "appServer", 28_420_000, false, true, 0),
    session("019f…a91", "Design token migration", "/Users/demo/Developer/atlas-web", "vscode", 15_760_000, false, false, 2),
    session("019e…01c", "Fix flaky integration tests", "/Users/demo/Developer/pulse-api", "cli", 8_940_000, false, false, 10),
    session("019d…3f2", "Release checklist", "/Users/demo/Developer/atlas-web", "cli", 5_120_000, true, false, 20),
    session("019c…9b4", "Database indexing review", "/Users/demo/Developer/pulse-api", "vscode", 4_210_000, true, false, 40),
  ];
  if (kind === "claudeCode") {
    sessions.forEach((record) => { record.archived = false; });
  }
  sessions[1].descendantIds = [sessions[3].id];
  sessions[3].parentThreadId = sessions[1].id;
  let items: CleanupItem[] = [
    ...sessions.map((record, index) => ({
      id: `session:${record.id}`,
      category: (record.archived ? "archivedSession" : "session") as StorageCategory,
      title: record.name,
      subtitle: record.cwd || undefined,
      paths: [kind === "codex" ? `/Users/demo/.codex/sessions/${record.id}.jsonl` : `/Users/demo/.claude/projects/demo/${record.id}.jsonl`],
      projectId: index === 0 ? undefined : index % 2 ? "atlas" : "pulse",
      threadId: record.id,
      sizeBytes: record.sizeBytes,
      modifiedAt: record.updatedAt,
      risk: "high" as const,
      recoverable: true,
      defaultSelected: false,
      protected: false,
      blockedReason: index === 0 ? `Thread is active or loaded in ${kind === "codex" ? "Codex" : "Claude Code"}` : undefined,
      metadata: { source: record.source, pinned: String(record.pinned) },
    })),
    {
      id: "memory:global",
      category: "memory" as const,
      title: "Global Codex memory",
      subtitle: "Merged across projects",
      paths: ["/Users/demo/.codex/memories_1.sqlite"],
      sizeBytes: 12_600_000,
      risk: "high" as const,
      recoverable: true,
      defaultSelected: false,
      protected: false,
      metadata: { scope: "global", files: "1" },
    },
    {
      id: "generated:1",
      category: "generatedImage" as const,
      title: "Generated visuals",
      subtitle: "11 images and visualizations",
      paths: ["/Users/demo/.codex/generated_images/orphan"],
      sizeBytes: 36_200_000,
      risk: "review" as const,
      recoverable: true,
      defaultSelected: false,
      protected: false,
      metadata: { entries: "11", association: "orphaned" },
    },
    {
      id: "attachment:1",
      category: "attachment" as const,
      title: "Session attachment bundle",
      subtitle: "ZIP archive",
      paths: ["/Users/demo/.codex/attachments/session-files.zip"],
      sizeBytes: 14_800_000,
      risk: "review" as const,
      recoverable: true,
      defaultSelected: false,
      protected: false,
      metadata: { entries: "1", association: "orphaned" },
    },
    {
      id: "logs:database",
      category: "log" as const,
      title: "Codex diagnostic logs",
      subtitle: "Entries older than 7 days",
      paths: ["/Users/demo/.codex/logs_2.sqlite"],
      sizeBytes: 18_300_000,
      risk: "safe" as const,
      recoverable: false,
      defaultSelected: false,
      protected: false,
      metadata: { retentionDays: "7" },
    },
    {
      id: "cache:1",
      category: "cache" as const,
      title: "WebView cache",
      subtitle: "Allowlisted and regenerable",
      paths: ["/Users/demo/Library/Application Support/Codex/Cache"],
      sizeBytes: 82_400_000,
      risk: "safe" as const,
      recoverable: false,
      defaultSelected: false,
      protected: false,
      metadata: { regenerable: "true" },
    },
    {
      id: "temporary:1",
      category: "temporary" as const,
      title: "Expired temporary files",
      subtitle: "Older than 24 hours",
      paths: ["/Users/demo/.codex/.tmp-old"],
      sizeBytes: 7_800_000,
      risk: "safe" as const,
      recoverable: false,
      defaultSelected: false,
      protected: false,
      metadata: { olderThanHours: "24" },
    },
    {
      id: "protected:auth.json",
      category: "protected" as const,
      title: "auth.json",
      subtitle: undefined,
      paths: ["/Users/demo/.codex/auth.json"],
      sizeBytes: 2_000,
      risk: "protected" as const,
      recoverable: false,
      defaultSelected: false,
      protected: true,
      blockedReason: "Protected data",
      metadata: {},
    },
  ];
  if (kind === "claudeCode") {
    items = items
      .filter((item) => item.category !== "attachment" && item.category !== "generatedImage")
      .map((item) => {
        if (item.category === "memory") return {
          ...item,
          id: "memory:atlas",
          title: "atlas-web memory",
          subtitle: "Claude Code project auto memory",
          paths: ["/Users/demo/.claude/projects/-Users-demo-Developer-atlas-web/memory"],
          projectId: "atlas",
          metadata: { scope: "project", files: "3" },
        };
        if (item.category === "log") return {
          ...item,
          id: "claude-history",
          title: "Prompt history",
          subtitle: "Recognized Claude Code application data",
          paths: ["/Users/demo/.claude/history.jsonl"],
          recoverable: true,
          risk: "review" as const,
        };
        if (item.category === "cache") return {
          ...item,
          title: "Claude Code caches",
          paths: ["/Users/demo/.claude/cache"],
        };
        if (item.category === "temporary") return {
          ...item,
          title: "Claude Code temporary data",
          paths: ["/Users/demo/.claude/plans"],
        };
        if (item.category === "protected") return {
          ...item,
          id: "protected:claude:settings.json",
          title: "settings.json",
          subtitle: "Claude Code configuration",
          paths: ["/Users/demo/.claude/settings.json"],
        };
        return item;
      });
  }
  const categories = ([
    "session",
    "archivedSession",
    "memory",
    "attachment",
    "generatedImage",
    "log",
    "cache",
    "temporary",
    "protected",
  ] as StorageCategory[])
    .map((category) => {
      const categoryItems = items.filter((item) => item.category === category);
      return {
        category,
        sizeBytes: categoryItems.reduce((total, item) => total + item.sizeBytes, 0),
        itemCount: categoryItems.length,
        defaultSelectedBytes: categoryItems
          .filter((item) => item.defaultSelected)
          .reduce((total, item) => total + item.sizeBytes, 0),
      };
    })
    .filter((category) => category.itemCount > 0);

  return {
    id: crypto.randomUUID(),
    scannedAt: new Date().toISOString(),
    installation: {
      kind,
      home: kind === "codex" ? "/Users/demo/.codex" : "/Users/demo/.claude",
      binary: kind === "codex" ? "/opt/homebrew/bin/codex" : "/Users/demo/.local/bin/claude",
      version: kind === "codex" ? "codex-cli 0.145.0" : "2.1.238 (Claude Code)",
      appSupport: kind === "codex" ? "/Users/demo/Library/Application Support/Codex" : undefined,
      running: true,
      capabilities: {
        threadList: true,
        threadDelete: true,
        memory: {
          canScan: true,
          canReadContent: true,
          canResetAll: kind === "codex",
          canResetScope: true,
          canEditEntries: false,
          canDeleteEntries: kind === "claudeCode",
          canToggleUse: false,
          canToggleGeneration: false,
          scope: kind === "codex" ? "global" : "project",
        },
        descendantFilter: true,
        reportOnly: false,
      },
      warnings: [],
    },
    totalBytes: items
      .filter((item) => item.category !== "protected")
      .reduce((total, item) => total + item.sizeBytes, 0),
    reclaimableBytes: items
      .filter((item) => item.defaultSelected)
      .reduce((total, item) => total + item.sizeBytes, 0),
    items,
    sessions,
    projects: [
      { id: "atlas", name: "atlas-web", roots: ["/Users/demo/Developer/atlas-web"], sessionIds: [sessions[1].id, sessions[3].id], sizeBytes: sessions[1].sizeBytes + sessions[3].sizeBytes },
      { id: "pulse", name: "pulse-api", roots: ["/Users/demo/Developer/pulse-api"], sessionIds: [sessions[2].id, sessions[4].id], sizeBytes: sessions[2].sizeBytes + sessions[4].sizeBytes },
    ],
    categories,
    warnings: [],
  };
}

function session(
  id: string,
  name: string,
  cwd: string,
  source: string,
  sizeBytes: number,
  archived = false,
  pinned = false,
  updatedDaysAgo = 0,
): SessionRecord {
  return {
    id,
    name,
    cwd,
    source,
    archived,
    pinned,
    status: pinned ? "loaded" : "notLoaded",
    createdAt: new Date(Date.now() - 12 * 86_400_000).toISOString(),
    updatedAt: new Date(Date.now() - updatedDaysAgo * 86_400_000).toISOString(),
    sizeBytes,
    descendantIds: [],
  };
}
