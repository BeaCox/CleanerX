import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  BackupRecord,
  CleanupItem,
  CleanupPlan,
  CleanupResult,
  InventorySnapshot,
  ItemContentDetail,
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
let mockSettings: AppSettings = {
  locale: "system",
  theme: "system",
  backupRetentionDays: 30,
  logRetentionDays: 7,
  tempRetentionHours: 24,
};

export const api = {
  async scanStorage(): Promise<InventorySnapshot> {
    if (inTauri()) return invoke("scan_storage");
    await delay(420);
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
        source: "appServer.thread/read",
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
        source: "recognizedMemoryDb.readOnly",
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
    return {
      itemId,
      source: "filesystem.readOnly",
      truncated: false,
      bytesRead: 72,
      blocks: [{ kind: "text", title: item.paths[0] ?? item.title, text: "Preview of the selected local item." }],
    };
  },

  async executeCleanup(planId: string, createBackup: boolean): Promise<CleanupResult> {
    if (inTauri()) return invoke("execute_cleanup", { planId, createBackup });
    await delay(850);
    const plan = mockPlans.get(planId);
    if (!createBackup && (plan?.estimatedBackupBytes ?? 0) > 0) {
      throw new Error("Recoverable data requires a verified encrypted backup");
    }
    return {
      operationId: planId,
      status: "complete",
      backupId: createBackup ? crypto.randomUUID() : undefined,
      reclaimedBytes: plan?.estimatedBytes ?? 0,
      deletedItemIds: [],
      warnings: [],
    };
  },

  async listBackups(): Promise<BackupRecord[]> {
    if (inTauri()) return invoke("list_backups");
    return [];
  },

  async restoreBackup(backupId: string): Promise<void> {
    if (inTauri()) await invoke("restore_backup", { backupId });
  },

  async purgeBackup(backupId: string): Promise<void> {
    if (inTauri()) await invoke("purge_backup", { backupId });
  },

  async getSettings(): Promise<AppSettings> {
    if (inTauri()) return invoke("get_settings");
    return structuredClone(mockSettings);
  },

  async updateSettings(settings: AppSettings): Promise<AppSettings> {
    if (inTauri()) return invoke("update_settings", { settings });
    mockSettings = structuredClone(settings);
    return structuredClone(settings);
  },
};

const delay = (milliseconds: number) =>
  new Promise((resolve) => window.setTimeout(resolve, milliseconds));

function createMockSnapshot(): InventorySnapshot {
  const sessions: SessionRecord[] = [
    session("019f…c47", "CleanerX", "/Users/demo/Developer/CleanerX", "appServer", 28_420_000, false, true),
    session("019f…a91", "Design token migration", "/Users/demo/Developer/atlas-web", "vscode", 15_760_000),
    session("019e…01c", "Fix flaky integration tests", "/Users/demo/Developer/pulse-api", "cli", 8_940_000),
    session("019d…3f2", "Release checklist", "/Users/demo/Developer/atlas-web", "cli", 5_120_000, true),
    session("019c…9b4", "Database indexing review", "/Users/demo/Developer/pulse-api", "vscode", 4_210_000, true),
  ];
  sessions[1].descendantIds = [sessions[3].id];
  sessions[3].parentThreadId = sessions[1].id;
  const items: CleanupItem[] = [
    ...sessions.map((record, index) => ({
      id: `session:${record.id}`,
      category: (record.archived ? "archivedSession" : "session") as StorageCategory,
      title: record.name,
      subtitle: record.cwd,
      paths: [`/Users/demo/.codex/sessions/${record.id}.jsonl`],
      projectId: index === 0 ? "cleanerx" : index % 2 ? "atlas" : "pulse",
      threadId: record.id,
      sizeBytes: record.sizeBytes,
      modifiedAt: record.updatedAt,
      risk: "high" as const,
      recoverable: true,
      defaultSelected: false,
      protected: false,
      blockedReason: index === 0 ? "Thread is active or loaded in Codex" : undefined,
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
      kind: "codex",
      home: "/Users/demo/.codex",
      binary: "/opt/homebrew/bin/codex",
      version: "codex-cli 0.145.0",
      appSupport: "/Users/demo/Library/Application Support/Codex",
      running: true,
      capabilities: {
        threadList: true,
        threadDelete: true,
        memoryReset: true,
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
      { id: "cleanerx", name: "CleanerX", roots: ["/Users/demo/Developer/CleanerX"], sessionIds: [sessions[0].id], sizeBytes: sessions[0].sizeBytes },
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
    updatedAt: new Date(Date.now() - Math.random() * 10 * 86_400_000).toISOString(),
    sizeBytes,
    descendantIds: [],
  };
}
