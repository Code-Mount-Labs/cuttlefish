import { it, expect, vi } from "vitest";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TaskActivity } from "../src/features/chat/activity";
import { DelegatedTasks } from "../src/features/chat/delegated-tasks";
import { ChatPage } from "../src/features/chat/chat-page";
import { activitySteps, elapsed } from "../src/lib/activity";
import { config, request, turn, workspace, json } from "./fixtures";
it("groups repeated work, preserves commentary, and exposes older activity on demand", () => {
  const now = Date.now();
  const events = Array.from({ length: 10 }, (_, i) => ({
    sequence: i + 1,
    command_id: request.command_id,
    phase: "commentary" as const,
    provider: "codex" as const,
    at_ms: now,
    summary: `Step ${i}`,
  }));
  const grouped = activitySteps([
    { ...events[0], phase: "working", summary: undefined },
    { ...events[0], phase: "waiting", summary: undefined },
    events[0],
    { ...events[0], sequence: 11 },
    ...events.slice(1),
  ]);
  expect(grouped[1].count).toBe(2);
  expect(elapsed(-1)).toBe("0s");
  expect(elapsed(61000)).toBe("1m 1s");
  render(<TaskActivity turn={turn} events={events} lastSeen={now} />);
  expect(screen.getAllByRole("listitem")).toHaveLength(6);
  fireEvent.click(screen.getByText("Show all 10 activities"));
  expect(screen.getAllByRole("listitem")).toHaveLength(10);
  fireEvent.click(screen.getByText("Show recent activity"));
  expect(screen.getAllByRole("listitem")).toHaveLength(6);
  expect(
    screen.getByText("Last activity 0s ago · Agent connected"),
  ).toBeInTheDocument();
});
it("updates the quiet-provider hint even when no new events arrive", () => {
  vi.useFakeTimers();
  const now = Date.now();
  const view = render(
    <TaskActivity
      turn={turn}
      events={[
        {
          sequence: 1,
          command_id: request.command_id,
          phase: "working",
          provider: "codex",
          at_ms: now,
        },
      ]}
      lastSeen={now + 120000}
    />,
  );
  act(() => vi.advanceTimersByTime(65000));
  expect(screen.getByText(/No new provider activity/)).toBeInTheDocument();
  view.unmount();
  vi.useRealTimers();
});
it("shows saved queued follow-ups and real child destinations", () => {
  const view = render(
    <TaskActivity
      turn={{
        ...turn,
        state: "queued",
        request: {
          ...request,
          action: {
            type: "send",
            message: { id: "m", text: "Follow up", attachments: [] },
          },
        },
      }}
      events={[]}
      lastSeen={null}
    />,
  );
  expect(
    screen.getByText("Queued · follows the current turn"),
  ).toBeInTheDocument();
  view.unmount();
  const open = vi.fn();
  const task = {
    task_id: "t",
    thread_id: "child",
    node_id: "n",
    state: "claimed" as const,
    title: "Build the iOS app",
    workspace: "Mac",
    provider: "codex" as const,
    platform: "mac_os",
  };
  const children = render(
    <DelegatedTasks
      tasks={[
        task,
        { ...task, task_id: "b", title: "Web", state: "completed" },
        {
          ...task,
          task_id: "failed",
          title: "Failed child",
          state: "completed",
          platform: "other",
          outcome: { type: "rejected" },
        },
        {
          ...task,
          task_id: "c",
          title: "Android",
          node_id: null,
          state: "planned",
          workspace: null,
          provider: null,
          platform: null,
        },
      ]}
      error={false}
      onOpen={open}
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Open task: Build the iOS app" }),
  );
  expect(open).toHaveBeenCalledWith("child");
  expect(
    screen.getByRole("button", { name: "Open task: Android" }),
  ).toBeDisabled();
  children.rerender(<DelegatedTasks tasks={[]} error onOpen={open} />);
  expect(screen.getByRole("status")).toHaveTextContent(
    "temporarily unavailable",
  );
});
it("accepts a follow-up during an active turn and sends it to the same thread", async () => {
  history.replaceState(null, "", `#chat/${request.thread_id}`);
  const submitted: unknown[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const path = String(input);
    if (path.endsWith("workspaces")) return json([{ workspace }]);
    if (path.endsWith("activity"))
      return json({ events: [], last_seen_ms: Date.now() });
    if (path.endsWith("tasks")) return json([]);
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      submitted.push(body);
      return json({ ...turn, request: body, state: "queued" });
    }
    if (path.endsWith("/chat"))
      return json([
        {
          thread_id: request.thread_id,
          workspace_id: workspace.workspace_id,
          title: "Work",
          request,
        },
      ]);
    return json([turn]);
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <ChatPage
        config={{
          ...config,
          features: {
            ...config.features,
            followups: true,
            collaboration: true,
          },
        }}
        active
        onSettings={() => {}}
        onLogout={() => {}}
      />
    </QueryClientProvider>,
  );
  const input = await screen.findByPlaceholderText("Add a follow-up…");
  fireEvent.change(input, { target: { value: "Use SwiftUI for the Mac app" } });
  fireEvent.click(screen.getByRole("button", { name: "Queue follow-up" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0]).toMatchObject({
    thread_id: request.thread_id,
    action: { type: "send", message: { text: "Use SwiftUI for the Mac app" } },
  });
});

it("shows grouped counts and unknown node liveness without claiming connection", () => {
  const event = {
    sequence: 1,
    command_id: request.command_id,
    phase: "reading_files" as const,
    provider: "codex" as const,
    at_ms: Date.now(),
  };
  render(
    <TaskActivity
      turn={turn}
      events={[event, { ...event, sequence: 2 }]}
      lastSeen={null}
    />,
  );
  expect(screen.getByText("×2")).toBeInTheDocument();
  expect(screen.queryByText(/Agent connected/)).toBeNull();
});
it("keeps a rejected message editable and allows navigating away", async () => {
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST")
      return json({ error: "No available agent" }, 400);
    if (String(input).endsWith("workspaces")) return json([{ workspace }]);
    return json([]);
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <ChatPage
        config={config}
        active
        onSettings={() => {}}
        onLogout={() => {}}
      />
    </QueryClientProvider>,
  );
  const input = screen.getByRole("textbox", { name: "Message Cuttlefish" });
  await waitFor(() =>
    expect(screen.getByLabelText("Workspace and provider")).toHaveValue(
      workspace.workspace_id,
    ),
  );
  fireEvent.change(input, { target: { value: "Build my app" } });
  fireEvent.click(screen.getByRole("button", { name: "Send message" }));
  expect(await screen.findByText("No available agent")).toBeInTheDocument();
  expect(input).toHaveValue("Build my app");
  expect(input).not.toHaveAttribute("readonly");
});
it("shows an immediate attention hint without a stale progress warning", () => {
  render(
    <TaskActivity
      turn={turn}
      events={[
        {
          sequence: 1,
          command_id: request.command_id,
          phase: "needs_attention",
          provider: "claude",
          at_ms: Date.now(),
        },
      ]}
      lastSeen={Date.now()}
    />,
  );
  expect(
    screen.getByText(/The provider stopped before confirming a result/),
  ).toBeInTheDocument();
  expect(screen.queryByText(/No new provider activity/)).toBeNull();
});
it("recovers from a rejected end-session request without locking the composer", async () => {
  history.replaceState(null, "", `#chat/${request.thread_id}`);
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const path = String(input);
    if (init?.method === "POST")
      return json({ error: "Session cannot end yet" }, 400);
    if (path.endsWith("workspaces")) return json([{ workspace }]);
    if (path.endsWith("activity"))
      return json({ events: [], last_seen_ms: Date.now() });
    if (path.endsWith("/chat"))
      return json([
        {
          thread_id: request.thread_id,
          workspace_id: workspace.workspace_id,
          title: "Work",
          request,
        },
      ]);
    return json([{ ...turn, state: "completed" }]);
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <ChatPage
        config={config}
        active
        onSettings={() => {}}
        onLogout={() => {}}
      />
    </QueryClientProvider>,
  );
  const options = await screen.findByRole("button", {
    name: "Conversation options",
  });
  fireEvent.pointerDown(options, {
    button: 0,
    ctrlKey: false,
    pointerType: "mouse",
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "End session" }));
  expect(await screen.findByText("Session cannot end yet")).toBeInTheDocument();
  expect(
    screen.getByRole("textbox", { name: "Message Cuttlefish" }),
  ).not.toHaveAttribute("readonly");
});

it("shows the precise interruption without hiding completed activity", () => {
  render(
    <TaskActivity
      turn={turn}
      events={[
        {
          sequence: 1,
          command_id: request.command_id,
          phase: "needs_attention",
          provider: "codex",
          at_ms: Date.now(),
          summary: "The provider connection closed.",
        },
      ]}
      lastSeen={Date.now()}
    />,
  );
  expect(
    screen.getByText(/The provider connection closed. Work already saved/),
  ).toBeInTheDocument();
});
