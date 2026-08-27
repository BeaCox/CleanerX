import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";

describe("CleanerX GUI", () => {
  it("keeps every cleanup item unselected after scanning", async () => {
    render(<App />);
    expect(await screen.findByText("Managed data")).toBeVisible();
    expect(screen.queryByText(/selected$/)).not.toBeInTheDocument();
    expect(screen.queryByText("Recommended")).not.toBeInTheDocument();
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
    expect(screen.queryByText("Scanning…")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Scanning…" })).not.toBeInTheDocument();
  });

  it("previews and persists interface language and appearance", async () => {
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

    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));
    expect(await screen.findByText("设置已保存")).toBeVisible();
    first.unmount();

    render(<App />);
    expect(await screen.findByText("可管理数据")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByRole("button", { name: "中文" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "深色" })).toHaveAttribute("aria-pressed", "true");
    expect(document.documentElement.dataset.theme).toBe("dark");

    fireEvent.click(screen.getByRole("button", { name: "浅色" }));
    fireEvent.click(screen.getByRole("button", { name: "English" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    expect(await screen.findByRole("button", { name: "概览" })).toBeVisible();
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("filters sessions before a detail is opened", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    const search = screen.getByRole("textbox", { name: "Search sessions or paths…" });
    fireEvent.change(search, { target: { value: "pulse-api" } });
    expect(screen.getByText("Fix flaky integration tests")).toBeVisible();
    expect(screen.getByText("Database indexing review")).toBeVisible();
    expect(screen.queryByText("Design token migration")).not.toBeInTheDocument();
  });

  it("keeps backup optional and warns about irreversible cleanup", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Design token migration" }));
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

  it("keeps active sessions unavailable for selection", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    expect(screen.getByRole("checkbox", { name: "CleanerX" })).toBeDisabled();
  });

  it("uses a project/session tree by default and preserves filtered ancestors", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    expect(screen.queryByRole("button", { name: "Projects" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Collapse atlas-web" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Collapse Design token migration" })).toBeVisible();
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
    expect(screen.getByText("Design token migration")).toBeVisible();
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeDisabled();
    expect(screen.getByText(/Ancestor of a filtered result/)).toBeVisible();
  });

  it("retains the flat list as an explicit alternate view", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    expect(screen.getByRole("columnheader", { name: "Project" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Collapse atlas-web" })).not.toBeInTheDocument();
    expect(screen.getByText("Design token migration")).toHaveAttribute("title", "Design token migration");
    expect(screen.getAllByText("Desktop / IDE")[1]).toHaveAttribute("title", "Desktop / IDE");
  });

  it("maps the vscode source to Desktop / IDE", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));

    expect(screen.getByRole("option", { name: "Desktop / IDE" })).toHaveValue("vscode");
    expect(screen.getAllByText("Desktop / IDE").length).toBeGreaterThan(1);
  });

  it("selects and clears every cleanable item in the current filter", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search sessions or paths…" }), { target: { value: "atlas-web" } });

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
    fireEvent.click(screen.getByLabelText("Open details Design token migration"));
    let dialog = screen.getByRole("dialog", { name: "Design token migration" });
    expect(within(dialog).getByText("Session information")).toBeVisible();
    expect(within(dialog).getAllByText("/Users/demo/Developer/atlas-web", { selector: "code" }).length).toBeGreaterThan(0);
    expect(await within(dialog).findByText("把项目与会话整理为树状视图，并保留项目根目录。")).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));

    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    fireEvent.click(screen.getByLabelText("Open details Global Codex memory"));
    dialog = screen.getByRole("dialog", { name: "Global Codex memory" });
    expect(within(dialog).getByText("Global memory")).toBeVisible();
    expect(within(dialog).getByText("/Users/demo/.codex/memories_1.sqlite", { selector: "code" })).toBeVisible();
    expect(await within(dialog).findByText(/用户偏好简洁的桌面界面/)).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));

    fireEvent.click(screen.getByRole("button", { name: "Attachments & generated" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Attachments & generated" }));

    expect(await screen.findByRole("img", { name: "Image preview for Generated visuals" })).toBeVisible();
    expect(await screen.findByLabelText("No image preview available")).toBeVisible();
    expect(screen.queryByText("No image preview available")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Open details Generated visuals")).toHaveClass("media-card");
  });

  it("does not open media details when the visual checkbox is clicked", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: "Attachments & generated" }));
    const checkbox = screen.getByRole("checkbox", { name: "Generated visuals" });
    const checkPath = checkbox.closest("label")?.querySelector("svg path");
    expect(checkPath).not.toBeNull();

    fireEvent.click(checkPath!);

    expect(checkbox).toBeChecked();
    expect(screen.queryByRole("dialog", { name: "Generated visuals" })).not.toBeInTheDocument();
  });

  it("permanently deletes a backup after an in-app confirmation", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(await screen.findByRole("button", { name: /Backups 1/ }));
    expect(screen.getByText("7f9fd849-9817-46ef-b075-7a437d32b03c")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Delete forever" }));
    const dialog = screen.getByRole("dialog", { name: "Permanently delete this backup?" });
    expect(within(dialog).getByText(/does not remove or modify current Codex data/)).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete backup forever" }));

    expect(await screen.findByText("No CleanerX backups yet")).toBeVisible();
    expect(screen.getByText("Backup permanently deleted")).toBeVisible();
  });
});
