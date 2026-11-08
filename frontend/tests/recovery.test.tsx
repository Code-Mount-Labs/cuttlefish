import { expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TaskRecovery } from "../src/features/chat/recovery";
import { ChatPage } from "../src/features/chat/chat-page";
import { config, request, turn, workspace, json } from "./fixtures";
const review = {
  id: "review",
  command_id: request.command_id,
  state: "ready",
  checkpoint: {
    provider: "claude",
    session_id: "saved",
    files: 3,
    progress: "Created the API",
    digest: "a".repeat(64),
  },
  detail: null,
};
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <TaskRecovery thread="thread" command={request.command_id} />
    </QueryClientProvider>,
  );
  return client;
}
it("inspects first and continues only the reviewed checkpoint", async () => {
  let view: unknown[] = [];
  const sent: Record<string, unknown>[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      sent.push(body);
      view = [
        { ...review, state: body.type === "inspect" ? "ready" : "approved" },
      ];
      return json(view[0]);
    }
    return json(view);
  });
  const client = mount();
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Inspect saved work" }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Inspect saved work" }));
  await screen.findByText(/3 project files found/);
  expect(screen.getByText(/Created the API/)).toBeInTheDocument();
  fireEvent.click(
    screen.getByRole("button", { name: "Continue from checkpoint" }),
  );
  await screen.findByText(
    "Continuation approved. Waiting for the original agent…",
  );
  expect(sent[1]).toEqual({
    type: "continue",
    id: "review",
    digest: review.checkpoint.digest,
  });
  view = [{ ...review, state: "resumed" }];
  await client.invalidateQueries();
  await screen.findByText(/Continuation saved/);
});
it("retains retry identity, shows blocking reasons and permits reinspection", async () => {
  let count = 0;
  let rows: unknown[] = [];
  const ids: string[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      ids.push(body.id);
      if (count++ === 0) throw Error("Connection lost");
      rows = [
        {
          ...review,
          state: "blocked",
          checkpoint: null,
          detail: "Previous provider may still be active.",
        },
      ];
      return json(rows[0]);
    }
    return json(rows);
  });
  const client = mount();
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Inspect saved work" }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Inspect saved work" }));
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "Inspect saved work" }));
  await screen.findByText("Previous provider may still be active.");
  expect(ids[0]).toBe(ids[1]);
  fireEvent.click(screen.getByRole("button", { name: "Inspect again" }));
  await waitFor(() => expect(ids).toHaveLength(3));
  expect(ids[2]).not.toBe(ids[1]);
  rows = [{ ...review, state: "inspecting", checkpoint: null }];
  await client.invalidateQueries();
  await screen.findByText(
    "Waiting for the original agent to inspect saved state…",
  );
});
it("reports fetch failures and handles checkpoints without commentary", async () => {
  let failed = true;
  vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
    failed
      ? json({ error: "Recovery unavailable" }, 503)
      : json([
          {
            ...review,
            checkpoint: {
              ...review.checkpoint,
              provider: "codex",
              progress: null,
            },
          },
        ]),
  );
  const client = mount();
  await screen.findByText("Recovery unavailable");
  failed = false;
  await client.invalidateQueries();
  await screen.findByText(/Codex · Saved session/);
});
it("places recovery controls only on an interrupted turn in a supporting installation", async () => {
  history.replaceState(null, "", "/#chat/" + request.thread_id);
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const url = String(input);
    if (url.endsWith("/recovery")) return json([]);
    if (url.endsWith("/activity"))
      return json({
        events: [
          {
            sequence: 1,
            command_id: request.command_id,
            provider: "codex",
            phase: "needs_attention",
            at_ms: Date.now(),
          },
        ],
        last_seen_ms: Date.now(),
      });
    if (url.endsWith("/workspaces"))
      return json([{ workspace, node_id: "node" }]);
    if (url.endsWith("/chat/" + request.thread_id))
      return json([{ ...turn, state: "claimed", outcome: null }]);
    return json([]);
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ChatPage
        config={{ ...config, features: { activity: true, recovery: true } }}
        active
        onSettings={() => {}}
        onLogout={() => {}}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("region", { name: "Recover interrupted task" });
});
