import { it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AutomationPage } from "../src/features/automations/automation-page";
import { ChatPage } from "../src/features/chat/chat-page";
import { config, workspace, json } from "./fixtures";

const workflow = {
  id: "00000000-0000-0000-0000-000000000101",
  revision: 1,
  name: "Review",
  workspace_id: workspace.workspace_id,
  instructions: "Review this project",
};
const route = {
  id: "00000000-0000-0000-0000-000000000102",
  revision: 1,
  name: "Morning",
  workflow_id: workflow.id,
  enabled: true,
  first_run_ms: Date.now() + 3600000,
  interval_minutes: 1440,
  next_run_ms: Date.now() + 3600000,
};
const run = {
  id: "00000000-0000-0000-0000-000000000103",
  workflow_id: workflow.id,
  route_id: route.id,
  thread_id: "00000000-0000-0000-0000-000000000103",
  created_ms: Date.now(),
  state: "completed",
  error: null,
};
function mount(node: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{node}</QueryClientProvider>,
  );
}
it("creates and edits reusable workflows, retries the same run identity, and opens run history", async () => {
  const user = userEvent.setup(),
    open = vi.fn(),
    bodies: Record<string, unknown>[] = [];
  let saved = false,
    fail = true;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      bodies.push(body);
      if (body.type === "run" && fail) {
        fail = false;
        return json({ error: "Connection lost" }, 503);
      }
      saved = true;
      return json({ ok: true });
    }
    return String(input).endsWith("workspaces")
      ? json([{ workspace, node_id: "node" }])
      : json({
          workflows: saved ? [workflow] : [],
          routes: [],
          runs: saved ? [run] : [],
        });
  });
  mount(<AutomationPage section="workflows" onOpenChat={open} />);
  expect(await screen.findByText(/Save instructions as/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "New workflow" }));
  await user.type(screen.getByLabelText("Name"), "Review");
  await user.type(screen.getByLabelText("Instructions"), "Review this project");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(
    await screen.findByRole("button", { name: "Edit Review" }),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Edit Review" }));
  expect(screen.getByLabelText("Instructions")).toHaveValue(
    workflow.instructions,
  );
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Run" }));
  expect(await screen.findAllByText("Connection lost")).not.toHaveLength(0);
  await user.click(screen.getByRole("button", { name: "Run" }));
  expect(await screen.findByText(/Review queued/)).toBeVisible();
  const runs = bodies.filter((b) => b.type === "run");
  expect(runs[0]).toEqual(runs[1]);
  await user.click(screen.getByRole("button", { name: "Open chat" }));
  expect(open).toHaveBeenCalledWith(run.thread_id);
});
it("creates schedules, edits their timing, pauses and resumes them", async () => {
  const user = userEvent.setup();
  let saved = false,
    enabled = true;
  const bodies: Record<string, unknown>[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      bodies.push(body);
      enabled = body.route.enabled;
      saved = true;
      return json({ ok: true });
    }
    return String(input).endsWith("workspaces")
      ? json([{ workspace, node_id: "node" }])
      : json({
          workflows: [workflow],
          routes: saved ? [{ ...route, enabled }] : [],
          runs: [{ ...run, state: "pending", error: "Waiting for agent" }],
        });
  });
  mount(<AutomationPage section="routes" onOpenChat={vi.fn()} />);
  expect(await screen.findByText(/Schedule a workflow/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "New route" }));
  await user.type(screen.getByLabelText("Name"), "Morning");
  await user.selectOptions(screen.getByLabelText("Repeat"), "60");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByRole("button", { name: "Pause" })).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Edit Morning" }));
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Pause" }));
  expect(await screen.findByText("Paused")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Resume" }));
  expect(await screen.findByRole("button", { name: "Pause" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Open chat" })).toBeDisabled();
  expect(screen.getByText("Waiting for agent")).toBeVisible();
  expect(bodies[0]).toMatchObject({
    type: "save_route",
    route: { interval_minutes: 60 },
  });
});
it("handles empty catalogs, missing workspaces, failed saves and dialog dismissal", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) =>
    init?.method === "POST"
      ? json({ error: "Stale revision" }, 409)
      : String(input).endsWith("workspaces")
        ? json([])
        : json({
            workflows: [{ ...workflow, workspace_id: crypto.randomUUID() }],
            routes: [],
            runs: [
              { ...run, workflow_id: crypto.randomUUID(), route_id: null },
            ],
          }),
  );
  mount(<AutomationPage section="workflows" onOpenChat={vi.fn()} />);
  expect(await screen.findByText("Workspace unavailable")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "New workflow" }));
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Edit Review" }));
  // A saved workspace can disappear. The browser requires selecting an available one before saving.
  expect(screen.getByLabelText("Agent workspace")).toHaveProperty("value", "");
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "Run" }));
  expect(await screen.findAllByText("Stale revision")).not.toHaveLength(0);
});
it("exposes the primary navigation rail and an empty Plugins section", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("automations")
      ? json({ workflows: [], routes: [], runs: [] })
      : json([]),
  );
  mount(
    <ChatPage config={config} active onLogout={vi.fn()} onSettings={vi.fn()} />,
  );
  await user.click(screen.getByRole("button", { name: "Routes" }));
  expect(await screen.findByText(/Create a workflow first/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Plugins" }));
  expect(screen.getByText("No plugins installed.")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Chats" }));
  expect(screen.getByRole("button", { name: "Chats" })).toHaveAttribute(
    "aria-current",
    "page",
  );
});

it.each(["automations", "workspaces"])(
  "reports %s read failures",
  async (failed) => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
      String(input).endsWith(failed)
        ? json({ error: "Read unavailable" }, 503)
        : String(input).endsWith("workspaces")
          ? json([])
          : json({ workflows: [], routes: [], runs: [] }),
    );
    mount(<AutomationPage section="workflows" onOpenChat={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Read unavailable",
    );
  },
);
it("keeps the editor open during saving and exposes a failed save for retry", async () => {
  const user = userEvent.setup();
  let resolve!: (value: Response) => void;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST")
      return new Promise<Response>((done) => {
        resolve = done;
      });
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ workflows: [workflow], routes: [], runs: [] });
  });
  mount(<AutomationPage section="workflows" onOpenChat={vi.fn()} />);
  await user.click(await screen.findByRole("button", { name: "Edit Review" }));
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByRole("button", { name: "Saving…" })).toBeDisabled();
  await user.keyboard("{Escape}");
  expect(screen.getByRole("dialog")).toBeVisible();
  resolve(json({ error: "Save failed" }, 503));
  expect(await screen.findByRole("alert")).toHaveTextContent("Save failed");
});
it("opens a workflow result through the primary navigation into its chat", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("automations")
      ? json({ workflows: [workflow], routes: [], runs: [run] })
      : json([]),
  );
  mount(
    <ChatPage config={config} active onLogout={vi.fn()} onSettings={vi.fn()} />,
  );
  await user.click(screen.getByRole("button", { name: "Workflows" }));
  await user.click(await screen.findByRole("button", { name: "Open chat" }));
  expect(screen.getByRole("button", { name: "Chats" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  expect(location.hash).toBe(`#chat/${run.thread_id}`);
});

it("preserves an API-created custom interval when opening the schedule editor", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({
          workflows: [workflow],
          routes: [{ ...route, interval_minutes: 15 }],
          runs: [],
        }),
  );
  mount(<AutomationPage section="routes" onOpenChat={vi.fn()} />);
  expect(await screen.findByText(/Every 15 minutes/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Edit Morning" }));
  expect(screen.getByLabelText("Repeat")).toHaveValue("15");
});
