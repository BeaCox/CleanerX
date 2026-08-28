import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "./App";
import { api } from "./api";
import type { AgentInstallation, CleanupItem, InventorySnapshot, RecoveryInventory, SessionRecord } from "./types";

function chooseMenuOption(label: string, option: string) {
  const trigger = screen.getByRole("combobox", { name: label });
  fireEvent.click(trigger);
  fireEvent.click(screen.getByRole("option", { name: option }));
  return trigger;
}

describe("CleanerX GUI", () => {
  it("uses the CleanerX artwork in the toolbar brand", () => {
    const { container } = render(<App />);
    const logo = container.querySelector<HTMLImageElement>(".toolbar-brand .brand-mark img");
    expect(logo).toBeInTheDocument();
    expect(logo?.getAttribute("src")).toContain("64x64.png");
  });

  it("presents attachments and generated items as file content", () => {
    render(<App />);
    const content = screen.getByRole("button", { name: "Content" });
    expect(content.querySelector(".lucide-file")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Attachments & generated" })).not.toBeInTheDocument();
  });

  it("keeps every cleanup item unselected after scanning", async () => {
    render(<App />);
    expect(await screen.findByText("Managed data")).toBeVisible();
    expect(screen.queryByText(/selected$/)).not.toBeInTheDocument();
    expect(screen.queryByText("Recommended")).not.toBeInTheDocument();
  });

  it("switches and persists the target Agent from the bottom status deck", async () => {
    const first = render(<App />);
    await screen.findByText("Managed data");
    const switcher = screen.getByRole("combobox", { name: "Target Agent" });
    expect(switcher).toHaveAttribute("data-value", "codex");
    expect(screen.getByText("Agent", { selector: ".status-agent-label" })).toBeVisible();

    chooseMenuOption("Target Agent", "Claude Code");

    expect(screen.getByText("Storage breakdown")).toBeVisible();
    expect(screen.getAllByText("Waiting for scan data").length).toBeGreaterThan(0);
    expect(await screen.findByText("Switched to Claude Code")).toBeVisible();
    expect(switcher).toHaveAttribute("data-value", "claudeCode");
    expect(screen.getByText("2.1.238")).toBeVisible();
    first.unmount();

    render(<App />);
    await screen.findByText("Managed data");
    expect(screen.getByRole("combobox", { name: "Target Agent" })).toHaveAttribute("data-value", "claudeCode");
  });

  it("exposes OpenCode as a target without claiming unsupported memory cleanup", async () => {
    render(<App />);
    await screen.findByText("Managed data");

    chooseMenuOption("Target Agent", "OpenCode");

    expect(await screen.findByText("Switched to OpenCode")).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Target Agent" })).toHaveAttribute("data-value", "openCode");
    expect(screen.getByText("1.18.3")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(await screen.findByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    expect(screen.getByText("No manageable data here yet")).toBeVisible();
    expect(screen.queryByText(/Reset clears|auto memory is project-scoped/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("textbox", { name: "OpenCode custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Codex custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Claude Code custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "pi custom directory" })).toBeVisible();
  });

  it("exposes pi as a target with file-backed sessions and no memory cleanup", async () => {
    render(<App />);
    await screen.findByText("Managed data");

    chooseMenuOption("Target Agent", "pi");

    expect(await screen.findByText("Switched to pi")).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Target Agent" })).toHaveAttribute("data-value", "pi");
    expect(screen.getByText("0.84.3")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(await screen.findByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    expect(screen.getByText("No manageable data here yet")).toBeVisible();
    expect(screen.queryByText(/Reset clears|auto memory is project-scoped/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("textbox", { name: "pi custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Codex custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Claude Code custom directory" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "OpenCode custom directory" })).toBeVisible();
  });

  it("uses detected installation state to hide unavailable Agent targets", async () => {
    const detected = await api.detectAgents();
    const unavailablePi: AgentInstallation[] = detected.map((installation) => installation.kind === "pi"
      ? { ...installation, state: "dataOnly", binary: undefined, version: undefined }
      : installation);
    const detect = vi.spyOn(api, "detectAgents").mockResolvedValue(unavailablePi);
    const update = vi.spyOn(api, "updateSettings");
    const view = render(<App />);
    try {
      await screen.findByText("Managed data");
      const switcher = screen.getByRole("combobox", { name: "Target Agent" });
      fireEvent.click(switcher);
      expect(screen.queryByRole("option", { name: "pi" })).not.toBeInTheDocument();
      expect(screen.getAllByRole("option")).toHaveLength(3);
      expect(screen.getByRole("listbox", { name: "Target Agent" })).not.toHaveTextContent("Installed");
      expect(switcher).toHaveAttribute("data-value", "codex");
      expect(update).not.toHaveBeenCalled();

      fireEvent.click(screen.getByRole("button", { name: "Settings" }));
      const piRow = screen.getByRole("textbox", { name: "pi custom directory" }).closest<HTMLElement>(".agent-setting-row")!;
      expect(within(piRow).getByText("Local data only")).toBeVisible();
      expect(screen.getByRole("textbox", { name: "pi custom directory" })).toBeEnabled();
    } finally {
      view.unmount();
      detect.mockRestore();
      update.mockRestore();
    }
  });

  it("refreshes all Agent installation states from settings", async () => {
    const installed = await api.detectAgents();
    const refreshed = installed.map((installation) => installation.kind === "claudeCode"
      ? { ...installation, state: "notFound" as const, binary: undefined, version: undefined }
      : installation);
    const detect = vi.spyOn(api, "detectAgents")
      .mockResolvedValueOnce(installed)
      .mockResolvedValueOnce(refreshed);
    const view = render(<App />);
    try {
      await screen.findByText("Managed data");
      fireEvent.click(screen.getByRole("button", { name: "Settings" }));
      const claudeRow = screen.getByRole("textbox", { name: "Claude Code custom directory" }).closest<HTMLElement>(".agent-setting-row")!;
      expect(within(claudeRow).getByText("Installed")).toBeVisible();

      fireEvent.click(screen.getByRole("button", { name: "Detect again" }));

      expect(await within(claudeRow).findByText("Not detected")).toBeVisible();
      expect(detect).toHaveBeenCalledTimes(2);
    } finally {
      view.unmount();
      detect.mockRestore();
    }
  });

  it("checks for signed application updates only after explicit user action", async () => {
    const status = vi.spyOn(api, "getAppUpdateStatus").mockResolvedValue({
      currentVersion: "0.1.0-alpha.1",
      support: "available",
    });
    const checkUpdate = vi.spyOn(api, "checkForAppUpdate").mockResolvedValue({
      currentVersion: "0.1.0-alpha.1",
      support: "available",
      update: {
        currentVersion: "0.1.0-alpha.1",
        version: "0.2.0",
        notes: "Signed release notes",
      },
    });
    const install = vi.spyOn(api, "installAppUpdate").mockImplementation(async (onEvent) => {
      onEvent({ event: "Started", data: { contentLength: 100 } });
      onEvent({ event: "Progress", data: { chunkLength: 100 } });
      onEvent({ event: "Finished" });
    });

    const view = render(<App />);
    try {
      await screen.findByText("Managed data");
      expect(checkUpdate).not.toHaveBeenCalled();

      fireEvent.click(screen.getByRole("button", { name: "Settings" }));
      expect(await screen.findByText("Current version 0.1.0-alpha.1")).toBeVisible();
      expect(status).toHaveBeenCalledTimes(1);
      expect(checkUpdate).not.toHaveBeenCalled();

      fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
      expect(await screen.findByText("CleanerX 0.2.0 is available")).toBeVisible();
      expect(screen.getByText("Signed release notes")).toBeVisible();

      fireEvent.click(screen.getByRole("button", { name: "Install 0.2.0" }));
      await waitFor(() => expect(install).toHaveBeenCalledTimes(1));
    } finally {
      view.unmount();
      status.mockRestore();
      checkUpdate.mockRestore();
      install.mockRestore();
    }
  });

  it("shows a draggable scrollbar only when the top view tabs overflow", () => {
    const view = render(<App />);
    const shell = view.container.querySelector<HTMLElement>(".view-tabs-shell")!;
    const tabs = view.container.querySelector<HTMLElement>(".view-tabs")!;
    expect(shell).toContainElement(tabs);
    expect(tabs).toHaveAttribute("aria-label", "Primary navigation");
    expect(screen.queryByRole("slider", { name: "Scroll page menu" })).not.toBeInTheDocument();

    Object.defineProperties(tabs, {
      clientWidth: { configurable: true, value: 240 },
      scrollWidth: { configurable: true, value: 760 },
      scrollLeft: { configurable: true, writable: true, value: 0 },
    });
    fireEvent(window, new Event("resize"));
    const scrollbar = screen.getByRole("slider", { name: "Scroll page menu" });
    expect(scrollbar).toHaveAttribute("max", "520");
    fireEvent.input(scrollbar, { target: { value: "130" } });
    expect(tabs.scrollLeft).toBe(130);
  });

  it("previews pi session files through the bounded read-only detail command", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    chooseMenuOption("Target Agent", "pi");
    await screen.findByText("Switched to pi");

    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(await screen.findByLabelText("Open details Design token migration"));

    const dialog = screen.getByRole("dialog", { name: "Design token migration" });
    expect(await within(dialog).findByText(/pi session file \(read-only\)/)).toBeVisible();
    expect(within(dialog).getByText("/Users/demo/.pi/agent/sessions/--Users-demo-Developer-atlas-web--/2026-08-26T09-30-00-000Z_019f…a91.jsonl", { selector: "code" })).toBeVisible();
    expect(within(dialog).getAllByText("pi", { selector: "header strong" }).length).toBeGreaterThan(0);
  });

  it("allows an inactive OpenCode session while keeping online backup unavailable", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    chooseMenuOption("Target Agent", "OpenCode");
    await screen.findByText("Switched to OpenCode");

    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(await screen.findByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
    const inactive = screen.getByRole("checkbox", { name: "Design token migration" });
    expect(inactive).toBeEnabled();
    fireEvent.click(inactive);
    fireEvent.click(screen.getByRole("button", { name: "Review cleanup" }));

    expect(await screen.findByRole("dialog", { name: "Review cleanup plan" })).toBeVisible();
    expect(screen.getByRole("checkbox", { name: /Create an encrypted backup first/ })).toBeDisabled();
    expect(screen.getByText(/Session backup is unavailable while OpenCode is running/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Clean without backup" })).toBeEnabled();
  });

  it("preserves the overview layout while scan data is pending", () => {
    render(<App />);
    expect(screen.getByText("Storage breakdown")).toBeVisible();
    expect(screen.getAllByText("Waiting for scan data").length).toBeGreaterThan(0);
  });

  it("renders storage usage as an accessible statistical chart", async () => {
    const { container } = render(<App />);
    await screen.findByText("Managed data");
    expect(screen.getByRole("img", { name: /Storage usage donut chart/ })).toBeVisible();
    const legendColors = new Map([...container.querySelectorAll<HTMLElement>(".category-row")].map((row) => [
      row.dataset.category,
      row.style.getPropertyValue("--category-color"),
    ]));
    [...container.querySelectorAll<SVGCircleElement>(".storage-donut-segment")].forEach((segment) => {
      expect(segment.style.getPropertyValue("--category-color")).toBe(legendColors.get(segment.dataset.category));
    });
    expect(screen.queryByText("Current manageable local storage grouped by data type")).not.toBeInTheDocument();
    expect(screen.queryByText("CleanerX · atlas-web")).not.toBeInTheDocument();
  });

  it("keeps settings usable while storage is still scanning", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByRole("button", { name: "Save settings" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Scanning…" })).toBeDisabled();
  });

  it("keeps the scan action available outside storage views", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("button", { name: "Scan again" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /^Backups/ }));
    expect(screen.getByRole("button", { name: "Scan again" })).toBeVisible();
  });

  it("previews and persists interface language, appearance, and text size", async () => {
    const first = render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    fireEvent.click(screen.getByRole("button", { name: "中文" }));
    expect(await screen.findByRole("button", { name: "保存设置" })).toBeVisible();
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");

    fireEvent.click(screen.getByRole("button", { name: "深色" }));
    expect(screen.getByRole("button", { name: "深色" })).toHaveAttribute("aria-pressed", "true");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");

    fireEvent.click(screen.getByRole("button", { name: "最大" }));
    expect(screen.getByRole("button", { name: "最大" })).toHaveAttribute("aria-pressed", "true");
    expect(document.documentElement.dataset.textSize).toBe("extraLarge");

    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));
    expect(await screen.findByText("设置已保存")).toBeVisible();
    first.unmount();

    render(<App />);
    expect(await screen.findByText("可管理数据")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByRole("button", { name: "中文" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "深色" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "最大" })).toHaveAttribute("aria-pressed", "true");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.textSize).toBe("extraLarge");

    fireEvent.click(screen.getByRole("button", { name: "浅色" }));
    fireEvent.click(screen.getByRole("button", { name: "English" }));
    fireEvent.click(screen.getByRole("button", { name: "Standard" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.textSize).toBe("standard");
    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    expect(await screen.findByRole("button", { name: "概览" })).toBeVisible();
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.textSize).toBe("extraLarge");
  });

  it("filters sessions before a detail is opened", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    const search = screen.getByRole("textbox", { name: "Search sessions or paths…" });
    fireEvent.change(search, { target: { value: "pulse-api" } });
    expect(await screen.findByText("Fix flaky integration tests")).toBeVisible();
    expect(screen.getByText("Database indexing review")).toBeVisible();
    expect(screen.queryByText("Design token migration")).not.toBeInTheDocument();
  });

  it("keeps backup optional and warns about irreversible cleanup", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    chooseMenuOption("Target Agent", "Claude Code");
    await screen.findByText("Switched to Claude Code");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(await screen.findByRole("checkbox", { name: "Design token migration" }));
    fireEvent.click(screen.getByRole("button", { name: "Review cleanup" }));
    expect(await screen.findByRole("dialog", { name: "Review cleanup plan" })).toBeVisible();
    const backup = screen.getByRole("checkbox", { name: /Create an encrypted backup first/ });
    expect(backup).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Clean without backup" })).toBeEnabled();
    expect(screen.getByText("Without a backup, deleted data cannot be restored.")).toBeVisible();
    fireEvent.click(backup);
    expect(screen.getByRole("button", { name: "Back up & clean" })).toBeEnabled();
    expect(screen.queryByText("Without a backup, deleted data cannot be restored.")).not.toBeInTheDocument();
  });

  it("warns when the selected mutation has no supported restore route", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(await screen.findByRole("checkbox", { name: "Design token migration" }));
    fireEvent.click(screen.getByRole("button", { name: "Review cleanup" }));

    expect(await screen.findByRole("dialog", { name: "Review cleanup plan" })).toBeVisible();
    expect(screen.queryByRole("checkbox", { name: /Create an encrypted backup first/ })).not.toBeInTheDocument();
    expect(screen.getByText(/has no supported restorable backup route/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Clean without backup" })).toBeEnabled();
  });

  it("keeps active sessions unavailable for selection", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(await screen.findByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
  });

  it("uses a project/session tree by default and preserves filtered ancestors", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    expect(screen.queryByRole("button", { name: "Projects" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Collapse atlas-web" })).toBeVisible();
    expect(await screen.findByRole("button", { name: "Collapse Design token migration" })).toBeVisible();
    const sessionTitle = screen.getByText("Design token migration");
    expect(sessionTitle).toHaveAttribute("title", "Design token migration");
    const sessionRow = sessionTitle.closest("tr")!;
    expect(within(sessionRow).getByText("Desktop / IDE").closest("td")).toHaveClass("session-col-source");
    expect(sessionRow.querySelector("td.session-col-updated")).toBeInTheDocument();
    expect(screen.getByText("Release checklist")).toBeVisible();
    expect(screen.queryByText("Expansion only changes the view, not cleanup selection.")).not.toBeInTheDocument();

    const collapseAll = screen.getByRole("button", { name: "Collapse all" });
    expect(collapseAll.querySelector("svg")).toBeInTheDocument();
    fireEvent.click(collapseAll);
    expect(screen.queryByText("Release checklist")).not.toBeInTheDocument();
    const expandAll = screen.getByRole("button", { name: "Expand all" });
    expect(expandAll.querySelector("svg")).toBeInTheDocument();
    fireEvent.click(expandAll);
    expect(screen.getByText("Release checklist")).toBeVisible();

    fireEvent.change(screen.getByRole("textbox", { name: "Search sessions or paths…" }), { target: { value: "Release checklist" } });
    expect(await screen.findByText("Design token migration")).toBeVisible();
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeDisabled();
    expect(screen.getByText(/Ancestor of a filtered result/)).toBeVisible();
  });

  it("keeps sessions without a project in a virtual root and filters recent updates by time", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    expect(screen.getByRole("button", { name: "Collapse No project" })).toBeVisible();
    expect(screen.getByRole("checkbox", { name: "Select sessions with no project" })).toBeDisabled();
    const projectFilter = screen.getByRole("combobox", { name: "Filter by project" });
    fireEvent.click(projectFilter);
    expect(screen.getByRole("option", { name: "No project" })).toHaveAttribute("aria-selected", "false");

    fireEvent.click(screen.getByRole("option", { name: "No project" }));
    expect(projectFilter).toHaveAttribute("data-value", "__no_project");
    expect(await screen.findByRole("checkbox", { name: "CleanerX" })).toBeVisible();
    expect(screen.queryByText("Design token migration")).not.toBeInTheDocument();

    chooseMenuOption("Filter by project", "All projects");
    chooseMenuOption("Filter by updated time", "Last 7 days");
    expect(await screen.findByText("Design token migration")).toBeVisible();
    expect(screen.queryByText("Fix flaky integration tests")).not.toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Open details CleanerX"));
    expect(within(screen.getByRole("dialog", { name: "CleanerX" })).getByText("No working directory")).toBeVisible();
  });

  it("retains the flat list as an explicit alternate view", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    expect(screen.getByRole("columnheader", { name: "Project" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Collapse atlas-web" })).not.toBeInTheDocument();
    expect(await screen.findByText("Design token migration")).toHaveAttribute("title", "Design token migration");
    expect(screen.getAllByText("Desktop / IDE")[1]).toHaveAttribute("title", "Desktop / IDE");
  });

  it("maps the vscode source to Desktop / IDE", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    fireEvent.click(screen.getByRole("combobox", { name: "Filter by source" }));
    expect(screen.getByRole("option", { name: "Desktop / IDE" })).toHaveAttribute("aria-selected", "false");
    await screen.findByText("Design token migration");
    expect(screen.getAllByText("Desktop / IDE").length).toBeGreaterThan(1);
  });

  it("supports keyboard navigation in custom filter menus", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    const projectFilter = screen.getByRole("combobox", { name: "Filter by project" });
    fireEvent.keyDown(projectFilter, { key: "ArrowDown" });
    expect(projectFilter).toHaveAttribute("aria-expanded", "true");
    fireEvent.keyDown(projectFilter, { key: "ArrowDown" });
    fireEvent.keyDown(projectFilter, { key: "Enter" });

    expect(projectFilter).toHaveAttribute("data-value", "atlas");
    expect(projectFilter).toHaveAttribute("aria-expanded", "false");
    expect(await screen.findByText("Design token migration")).toBeVisible();
    expect(screen.queryByText("Fix flaky integration tests")).not.toBeInTheDocument();
  });

  it("offers an in-place retry when filtered session groups fail to load", async () => {
    const getProjects = vi.spyOn(api, "getSessionProjects").mockRejectedValue(new Error("temporary read failure"));
    const view = render(<App />);
    try {
      await screen.findByText("Managed data");
      fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
      fireEvent.change(screen.getByRole("textbox", { name: "Search sessions or paths…" }), { target: { value: "atlas" } });

      expect(await screen.findByRole("alert")).toHaveTextContent("Could not load session groups: temporary read failure");
      fireEvent.click(screen.getByRole("button", { name: "Retry" }));
      expect(getProjects).toHaveBeenCalledTimes(2);
    } finally {
      view.unmount();
      getProjects.mockRestore();
    }
  });

  it("keeps every project summary visible while lazily requesting bounded session pages", async () => {
    const base = await api.scanStorage("codex");
    const seed = await api.getSessionPage({ snapshotId: base.id, query: "", cursor: 0, limit: 100, includeAncestors: false });
    const large = createLargeSessionInventory(base, seed.sessions, seed.items, 161);
    const scan = vi.spyOn(api, "scanStorage").mockResolvedValue(large.report);
    let releaseFirstPage: (() => void) | undefined;
    const getPage = vi.spyOn(api, "getSessionPage").mockImplementation((request) => {
      const pageSessions = large.sessions.slice(request.cursor, request.cursor + request.limit);
      const includedIds = new Set(pageSessions.map((session) => session.id));
      const end = request.cursor + pageSessions.length;
      const result = {
        snapshotId: large.report.id,
        sessions: pageSessions,
        items: large.items.filter((item) => item.threadId && includedIds.has(item.threadId)),
        matchingSessionIds: pageSessions.map((session) => session.id),
        totalCount: large.sessions.length,
        nextCursor: end < large.sessions.length ? end : undefined,
      };
      if (request.cursor === 0) return new Promise((resolve) => { releaseFirstPage = () => resolve(result); });
      return Promise.resolve(result);
    });
    const intersection = installControllableIntersectionObserver();

    const view = render(<App />);
    try {
      await screen.findByText("Managed data");
      fireEvent.click(screen.getByRole("button", { name: /Sessions 161/ }));

      expect(screen.getByText("161 sessions")).toBeVisible();
      expect(getPage).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
      expect(screen.getByText("161 sessions")).toBeVisible();
      expect(getPage).not.toHaveBeenCalled();
      expect(screen.getByRole("checkbox", { name: "Select sessions with no project" })).toBeEnabled();
      fireEvent.click(screen.getByRole("button", { name: "Select all results" }));
      expect(screen.getByText("161 selected")).toBeVisible();
      expect(screen.getByRole("checkbox", { name: "Select sessions with no project" })).toBeChecked();
      expect(getPage).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Deselect all results" }));

      fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
      expect(intersection.visibleTargets().every((target) => !target.classList.contains("session-lazy-row"))).toBe(true);
      act(() => intersection.triggerVisible());
      expect(screen.getByRole("table")).toHaveAttribute("aria-busy", "true");
      expect(view.container.querySelector("tr.session-lazy-row")).toHaveAttribute("hidden");
      expect(screen.queryByText("Loading sessions…")).not.toBeInTheDocument();
      await act(async () => releaseFirstPage?.());
      expect(await screen.findByText("Bulk session 049")).toBeVisible();
      expect(screen.queryByText("Bulk session 050")).not.toBeInTheDocument();
      expect(view.container.querySelector("tr.session-lazy-row")).toHaveAttribute("hidden");
      expect(getPage).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: 0, limit: 50, projectId: "__no_project" }));

      act(() => intersection.triggerVisible());
      expect(await screen.findByText("Bulk session 099")).toBeVisible();
      expect(screen.queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
      expect(getPage).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: 50, limit: 50 }));
    } finally {
      view.unmount();
      intersection.restore();
      getPage.mockRestore();
      scan.mockRestore();
    }
  });

  it("selects and clears every cleanable item in the current filter", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search sessions or paths…" }), { target: { value: "atlas-web" } });

    await screen.findByRole("checkbox", { name: "Design token migration" });
    fireEvent.click(screen.getByRole("button", { name: "Select all results" }));
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Release checklist" })).toBeChecked();
    expect(screen.getByRole("button", { name: "Deselect all results" })).toHaveClass("secondary-button", "bulk-select-button");

    expect(screen.queryByRole("button", { name: "Clear current results" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Deselect all results" }));
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Release checklist" })).not.toBeChecked();
  });

  it("selects only the sessions beneath a project group", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    await screen.findByRole("checkbox", { name: "Design token migration" });
    fireEvent.click(screen.getByRole("checkbox", { name: "Select project data atlas-web" }));
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Release checklist" })).toBeChecked();
    expect(within(screen.getByRole("row", { name: /Select project data atlas-web/ })).getAllByRole("cell")).toHaveLength(5);
    expect(screen.getByText("2 selected")).toBeVisible();
  });

  it("toggles current session results with Ctrl+A outside text fields", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    await screen.findByRole("checkbox", { name: "Design token migration" });
    fireEvent.keyDown(window, { key: "a", ctrlKey: true });
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Fix flaky integration tests" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
    fireEvent.keyDown(window, { key: "a", ctrlKey: true });
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Fix flaky integration tests" })).not.toBeChecked();
  });

  it("toggles the current non-session category with Ctrl+A", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Logs & caches" }));
    expect(screen.getByRole("checkbox", { name: "Codex diagnostic logs" })).not.toBeChecked();
    fireEvent.keyDown(window, { key: "a", ctrlKey: true });
    expect(screen.getByRole("checkbox", { name: "Codex diagnostic logs" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "WebView cache" })).toBeChecked();
    fireEvent.keyDown(window, { key: "a", ctrlKey: true });
    expect(screen.getByRole("checkbox", { name: "Codex diagnostic logs" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "WebView cache" })).not.toBeChecked();
  });

  it("loads and renders category-specific content only after details are opened", async () => {
    render(<App />);
    await screen.findByText("Managed data");

    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(screen.queryByRole("button", { name: /View details/ })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByLabelText("Open details Design token migration"));
    let dialog = screen.getByRole("dialog", { name: "Design token migration" });
    expect(within(dialog).getByText("Session information")).toBeVisible();
    expect(within(dialog).getAllByText("/Users/demo/Developer/atlas-web", { selector: "code" }).length).toBeGreaterThan(0);
    expect(await within(dialog).findByText("把项目与会话整理为树状视图，并保留项目根目录。")).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));

    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    fireEvent.click(screen.getByLabelText("Open details Global Codex memory"));
    dialog = screen.getByRole("dialog", { name: "Global Codex memory" });
    expect(within(dialog).getByText("Automatic memory")).toBeVisible();
    expect(within(dialog).getByText("/Users/demo/.codex/memories_1.sqlite", { selector: "code" })).toBeVisible();
    expect(await within(dialog).findByText(/用户偏好简洁的桌面界面/)).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));

    fireEvent.click(screen.getByRole("button", { name: "Content" }));
    fireEvent.click(screen.getByLabelText("Open details Generated visuals"));
    dialog = screen.getByRole("dialog", { name: "Generated visuals" });
    expect(within(dialog).getByText("Generated content")).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));

    fireEvent.click(screen.getByRole("button", { name: "Logs & caches" }));
    fireEvent.click(screen.getByLabelText("Open details Codex diagnostic logs"));
    dialog = screen.getByRole("dialog", { name: "Codex diagnostic logs" });
    expect(within(dialog).getByText("Retention days")).toBeVisible();
    expect(within(dialog).getByText("7")).toBeVisible();
    expect(await within(dialog).findByText("thread/list completed · 5 rows")).toBeVisible();
  });

  it("shows attachment and generated images in a preview card grid", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Content" }));

    expect(await screen.findByRole("img", { name: "Image preview for Generated visuals" })).toBeVisible();
    expect(await screen.findByLabelText("No image preview available")).toBeVisible();
    expect(screen.queryByText("No image preview available")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Open details Generated visuals")).toHaveClass("media-card");
  });

  it("keeps orphan media inspect-only without opening details from its disabled checkbox", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Content" }));
    const checkbox = screen.getByRole("checkbox", { name: "Generated visuals" });
    const checkPath = checkbox.closest("label")?.querySelector("svg path");
    expect(checkPath).not.toBeNull();

    expect(checkbox).toBeDisabled();
    fireEvent.click(checkPath!);

    expect(checkbox).not.toBeChecked();
    expect(screen.queryByRole("dialog", { name: "Generated visuals" })).not.toBeInTheDocument();
  });

  it("permanently deletes a backup after an in-app confirmation", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(await screen.findByRole("button", { name: /Backups 1/ }));
    expect(screen.getByText("7f9fd849-9817-46ef-b075-7a437d32b03c")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Delete forever" }));
    const dialog = screen.getByRole("dialog", { name: "Permanently delete this backup?" });
    expect(within(dialog).getByText(/does not remove or modify current Agent data/)).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete backup forever" }));

    expect(await screen.findByText("No CleanerX backups yet")).toBeVisible();
    expect(screen.getByText("Backup permanently deleted")).toBeVisible();
  });

  it("prompts for a rescanned operation and allows it to be reconciled", async () => {
    const recovery: RecoveryInventory = {
      operations: [{
        operationId: "4b8779cf-19ee-4c67-9488-5bb2749650c8",
        agent: "codex",
        journalStatus: "verifying",
        updatedAt: new Date().toISOString(),
        backupId: "7f9fd849-9817-46ef-b075-7a437d32b03c",
        completedMutations: 1,
        totalMutations: 1,
        observedAppliedMutations: 1,
        observation: "applied",
        canFinalize: true,
        canRestore: false,
        canTerminate: true,
        reason: "Quit Codex before restoring the committed backup",
      }],
      warnings: [],
    };
    const list = vi.spyOn(api, "listRecoveryOperations")
      .mockResolvedValueOnce(recovery)
      .mockResolvedValue({ operations: [], warnings: [] });
    const reconcile = vi.spyOn(api, "reconcileRecoveryOperation").mockResolvedValue({
      ...recovery.operations[0],
      journalStatus: "complete",
      canFinalize: false,
    });
    try {
      render(<App />);
      const dialog = await screen.findByRole("dialog", { name: "Incomplete operation requires recovery" });
      expect(within(dialog).getByText("4b8779cf-19ee-4c67-9488-5bb2749650c8")).toBeVisible();
      expect(within(dialog).getByText("All applied")).toBeVisible();
      expect(within(dialog).getByRole("button", { name: "Restore verified backup" })).toBeDisabled();
      expect(within(dialog).getByRole("button", { name: "Continue browsing" })).toBeEnabled();

      fireEvent.click(within(dialog).getByRole("button", { name: "Accept verified result" }));

      await waitFor(() => expect(reconcile).toHaveBeenCalledWith(recovery.operations[0].operationId));
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Incomplete operation requires recovery" })).not.toBeInTheDocument());
    } finally {
      list.mockRestore();
      reconcile.mockRestore();
    }
  });

  it("opens recovery immediately when cleanup fails after journaling", async () => {
    const recovery: RecoveryInventory = {
      operations: [{
        operationId: "a0dd8117-fb63-4bc4-8bd7-d76eb93797f7",
        agent: "codex",
        journalStatus: "failed",
        updatedAt: new Date().toISOString(),
        completedMutations: 0,
        totalMutations: 1,
        observedAppliedMutations: 0,
        observation: "notApplied",
        canFinalize: false,
        canRestore: false,
        canTerminate: true,
        reason: "No independently verified backup is available",
      }],
      warnings: [],
    };
    const list = vi.spyOn(api, "listRecoveryOperations")
      .mockResolvedValueOnce({ operations: [], warnings: [] })
      .mockResolvedValue(recovery);
    const execute = vi.spyOn(api, "executeCleanup").mockRejectedValue(new Error("injected mutation failure"));
    try {
      render(<App />);
      await screen.findByText("Managed data");
      fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
      fireEvent.click(await screen.findByRole("checkbox", { name: "Design token migration" }));
      fireEvent.click(screen.getByRole("button", { name: "Review cleanup" }));
      fireEvent.click(await screen.findByRole("button", { name: "Clean without backup" }));

      expect(await screen.findByRole("dialog", { name: "Incomplete operation requires recovery" })).toBeVisible();
      expect(screen.getByText("a0dd8117-fb63-4bc4-8bd7-d76eb93797f7")).toBeVisible();
      expect(list).toHaveBeenCalledTimes(2);
    } finally {
      list.mockRestore();
      execute.mockRestore();
    }
  });

  it("allows an unrecognized recovery warning to be dismissed for browsing", async () => {
    const list = vi.spyOn(api, "listRecoveryOperations").mockResolvedValue({
      operations: [],
      warnings: ["operation journal format 99 is not recognized"],
    });
    try {
      render(<App />);
      const dialog = await screen.findByRole("dialog", { name: "Operation recovery record is unavailable" });
      expect(within(dialog).getByText(/can keep browsing, but new cleanup remains blocked/)).toBeVisible();
      expect(within(dialog).getByText("operation journal format 99 is not recognized")).toBeVisible();
      fireEvent.click(within(dialog).getByRole("button", { name: "Continue browsing" }));
      expect(screen.queryByRole("dialog", { name: "Operation recovery record is unavailable" })).not.toBeInTheDocument();
      expect(await screen.findByText("Managed data")).toBeVisible();
    } finally {
      list.mockRestore();
    }
  });
});

function createLargeSessionInventory(base: InventorySnapshot, seedSessions: SessionRecord[], seedItems: CleanupItem[], count: number) {
  const sessionTemplate = seedSessions.find((session) => !session.parentThreadId)!;
  const itemTemplate = seedItems.find((item) => item.threadId === sessionTemplate.id)!;
  const sessions: SessionRecord[] = Array.from({ length: count }, (_, index) => ({
    ...sessionTemplate,
    id: `bulk-session-${index.toString().padStart(3, "0")}`,
    name: `Bulk session ${index.toString().padStart(3, "0")}`,
    cwd: `/Users/demo/Developer/bulk/project-${index}`,
    archived: false,
    pinned: false,
    status: "notLoaded",
    updatedAt: new Date(Date.now() - index * 60_000).toISOString(),
    parentThreadId: undefined,
    descendantIds: [],
  }));
  const sessionItems: CleanupItem[] = sessions.map((session) => ({
    ...itemTemplate,
    id: `session:${session.id}`,
    category: "session",
    title: session.name,
    subtitle: session.cwd,
    paths: [`/Users/demo/.codex/sessions/${session.id}.jsonl`],
    projectId: undefined,
    threadId: session.id,
    modifiedAt: session.updatedAt,
    blockedReason: undefined,
  }));
  const otherItems = base.items.filter((item) => !item.threadId);
  const sessionBytes = sessionItems.reduce((sum, item) => sum + item.sizeBytes, 0);
  const report: InventorySnapshot = {
    ...base,
    id: crypto.randomUUID(),
    sessions: [],
    projects: [],
    items: otherItems,
    totalBytes: sessionBytes + otherItems.filter((item) => item.category !== "protected").reduce((sum, item) => sum + item.sizeBytes, 0),
    sessionCount: count,
    archivedSessionCount: 0,
    unassignedSessionCount: count,
    unassignedSessionSizeBytes: sessionBytes,
    sessionSelection: sessionItems.map((item) => ({ id: item.id, threadId: item.threadId!, sizeBytes: item.sizeBytes })),
    categories: base.categories
      .filter((category) => category.category !== "archivedSession")
      .map((category) => category.category === "session" ? { ...category, itemCount: count, sizeBytes: sessionBytes } : category),
  };
  return { report, sessions, items: sessionItems };
}

function installControllableIntersectionObserver() {
  const original = window.IntersectionObserver;
  const observers = new Set<TestIntersectionObserver>();
  class TestIntersectionObserver {
    active = true;
    callback: IntersectionObserverCallback;
    target?: Element;

    constructor(callback: IntersectionObserverCallback) {
      this.callback = callback;
      observers.add(this);
    }

    observe(target: Element) { this.target = target; }
    unobserve(target: Element) { if (this.target === target) this.target = undefined; }
    takeRecords() { return []; }
    disconnect() { this.active = false; }
  }
  window.IntersectionObserver = TestIntersectionObserver as unknown as typeof IntersectionObserver;
  return {
    visibleTargets() {
      return [...observers].filter((observer) => observer.active && observer.target).map((observer) => observer.target!);
    },
    triggerVisible() {
      [...observers].filter((observer) => observer.active).forEach((observer) => {
        observer.callback([{ isIntersecting: true } as IntersectionObserverEntry], observer as unknown as IntersectionObserver);
      });
    },
    restore() {
      if (original) window.IntersectionObserver = original;
      else delete (window as Window & { IntersectionObserver?: typeof IntersectionObserver }).IntersectionObserver;
    },
  };
}
