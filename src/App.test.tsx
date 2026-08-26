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

  it("keeps settings usable while storage is still scanning", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByText("CleanerX settings")).toBeVisible();
    expect(screen.queryByText("Scanning…")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Scanning…" })).not.toBeInTheDocument();
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

  it("uses a backup control instead of a separate approval checkbox", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Design token migration" }));
    fireEvent.click(screen.getByRole("button", { name: "Review cleanup" }));
    expect(await screen.findByRole("dialog", { name: "Review cleanup plan" })).toBeVisible();
    const backup = screen.getByRole("checkbox", { name: /Create an encrypted backup first/ });
    expect(backup).toBeChecked();
    expect(screen.getByRole("button", { name: "Back up & clean" })).toBeEnabled();
    fireEvent.click(backup);
    expect(screen.getByRole("button", { name: "Backup required" })).toBeDisabled();
    expect(screen.getByText(/verified encrypted backup is required/)).toBeVisible();
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
  });

  it("selects and clears every cleanable item in the current filter", async () => {
    render(<App />);
    await screen.findByText("Managed data");
    fireEvent.click(screen.getByRole("button", { name: /Sessions 5/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search sessions or paths…" }), { target: { value: "atlas-web" } });

    fireEvent.click(screen.getByRole("button", { name: "Select all results" }));
    expect(screen.getByRole("checkbox", { name: "Design token migration" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Release checklist" })).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "Clear current results" }));
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
});
