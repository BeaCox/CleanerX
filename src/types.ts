export type StorageCategory =
  | "session"
  | "archivedSession"
  | "memory"
  | "attachment"
  | "generatedImage"
  | "log"
  | "cache"
  | "temporary"
  | "protected";

export type RiskLevel = "safe" | "review" | "high" | "protected";
export type AgentKind = "codex" | "claudeCode" | "openCode" | "pi";
export type AgentDetectionState = "installed" | "dataOnly" | "notFound";

export interface MemoryCapabilities {
  canScan: boolean;
  canReadContent: boolean;
  canResetAll: boolean;
  canResetScope: boolean;
  canEditEntries: boolean;
  canDeleteEntries: boolean;
  canToggleUse: boolean;
  canToggleGeneration: boolean;
  scope: "global" | "project" | "mixed";
}

export interface AgentCapabilities {
  threadList: boolean;
  threadDelete: boolean;
  memory: MemoryCapabilities;
  descendantFilter: boolean;
  reportOnly: boolean;
}

export interface AgentInstallation {
  kind: AgentKind;
  state: AgentDetectionState;
  home: string;
  binary?: string;
  version?: string;
  appSupport?: string;
  running: boolean;
  capabilities: AgentCapabilities;
  warnings: string[];
}

export interface CleanupItem {
  id: string;
  category: StorageCategory;
  title: string;
  subtitle?: string;
  paths: string[];
  projectId?: string;
  threadId?: string;
  sizeBytes: number;
  modifiedAt?: string;
  risk: RiskLevel;
  recoverable: boolean;
  defaultSelected: boolean;
  protected: boolean;
  blockedReason?: string;
  metadata: Record<string, string>;
}

export interface SelectionCandidate {
  id: string;
  sizeBytes: number;
}

export type ContentBlock =
  | { kind: "message"; role: string; text: string; phase?: string }
  | { kind: "text"; title: string; text: string }
  | { kind: "image"; title: string; dataUrl: string }
  | { kind: "log"; timestamp?: string; level?: string; target?: string; text: string }
  | { kind: "notice"; text: string };

export interface ItemContentDetail {
  itemId: string;
  source: string;
  truncated: boolean;
  bytesRead: number;
  blocks: ContentBlock[];
  warning?: string;
}

export interface ItemThumbnail {
  itemId: string;
  title: string;
  dataUrl: string;
}

export interface SessionRecord {
  id: string;
  name: string;
  cwd: string;
  path?: string;
  source: string;
  archived: boolean;
  pinned: boolean;
  status: string;
  createdAt?: string;
  updatedAt?: string;
  sizeBytes: number;
  parentThreadId?: string;
  descendantIds: string[];
}

export interface ProjectGroup {
  id: string;
  name: string;
  roots: string[];
  sessionCount: number;
  sizeBytes: number;
  sessionIds?: string[];
}

export interface SessionFilter {
  snapshotId: string;
  projectId?: string;
  query: string;
  source?: string;
  state?: "active" | "archived";
  updatedWithinDays?: number;
}

export interface SessionProjectResult {
  projects: ProjectGroup[];
  unassignedSessionCount: number;
  unassignedSessionSizeBytes: number;
  selection: SessionSelectionCandidate[];
}

export interface SessionSelectionCandidate extends SelectionCandidate {
  threadId: string;
  projectId?: string;
}

export interface SessionPageRequest extends SessionFilter {
  cursor: number;
  limit: number;
  includeAncestors: boolean;
}

export interface SessionPage {
  snapshotId: string;
  sessions: SessionRecord[];
  items: CleanupItem[];
  matchingSessionIds: string[];
  totalCount: number;
  nextCursor?: number;
}

export interface CategorySummary {
  category: StorageCategory;
  sizeBytes: number;
  itemCount: number;
  defaultSelectedBytes: number;
}

export interface InventorySnapshot {
  id: string;
  scannedAt: string;
  installation: AgentInstallation;
  totalBytes: number;
  reclaimableBytes: number;
  items: CleanupItem[];
  sessions: SessionRecord[];
  projects: ProjectGroup[];
  categories: CategorySummary[];
  warnings: string[];
  sessionCount: number;
  archivedSessionCount: number;
  sessionSources: string[];
  unassignedSessionCount: number;
  unassignedSessionSizeBytes: number;
  sessionSelection: SessionSelectionCandidate[];
}

export interface PlannedOperation {
  kind: "deleteSession" | "resetMemory" | "cleanRegenerable";
  itemIds: string[];
  sessionIds: string[];
  paths: string[];
  sizeBytes: number;
  backupEligible: boolean;
  requiresAgentExit: boolean;
  blockers: string[];
}

export interface CleanupPlan {
  id: string;
  snapshotId: string;
  createdAt: string;
  selectedItemIds: string[];
  expandedSessionIds: string[];
  operations: PlannedOperation[];
  estimatedBytes: number;
  estimatedBackupBytes: number;
  blockers: string[];
}

export interface CleanupResult {
  operationId: string;
  status: string;
  backupId?: string;
  reclaimedBytes: number;
  deletedItemIds: string[];
  warnings: string[];
}

export interface BackupRecord {
  id: string;
  createdAt: string;
  expiresAt: string;
  archivePath: string;
  archiveBytes: number;
  originalBytes: number;
  itemCount: number;
  operationId: string;
  agent: AgentKind;
}

export interface AppSettings {
  activeAgent: AgentKind;
  customCodexHome?: string;
  customClaudeHome?: string;
  customOpencodeHome?: string;
  customPiHome?: string;
  locale: "system" | "zh" | "en";
  theme: "system" | "light" | "dark";
  textSize: "standard" | "large" | "extraLarge";
  backupRetentionDays: number;
  logRetentionDays: number;
  tempRetentionHours: number;
}

export type ViewId =
  | "overview"
  | "sessions"
  | "memory"
  | "generated"
  | "logs"
  | "backups"
  | "settings";
