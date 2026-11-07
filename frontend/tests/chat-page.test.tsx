import { it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChatPage } from "../src/features/chat/chat-page";
import { config, json, request, turn, workspace } from "./fixtures";
it("navigates chats, searches, preserves scroll position, and shows exact task status", async () => {
  const user = userEvent.setup(),
    cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let phase = "claimed";
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const path = String(input).replace("/api/local/", "");
    if (path === "automations")
      return json({ workflows: [], routines: [], routes: [], runs: [] });
    if (path === "chat/workspaces")
      return json([
        { workspace },
        {
          workspace: {
            ...workspace,
            workspace_id: "second",
            name: "Second",
            provider: "claude",
          },
        },
      ]);
    if (path === "chat" && init?.method === "POST") {
      phase = "ended";
      return json({ ...turn, request: JSON.parse(String(init.body)) });
    }
    if (path === "chat")
      return json([
        {
          thread_id: request.thread_id,
          workspace_id: workspace.workspace_id,
          title: null,
          request,
        },
      ]);
    if (path.endsWith("/activity"))
      return json({
        events: [
          {
            sequence: 1,
            command_id: request.command_id,
            provider: "claude",
            phase: "reading_files",
            at_ms: Date.now(),
          },
          {
            sequence: 2,
            command_id: "unrelated",
            provider: "codex",
            phase: "running_command",
            at_ms: Date.now(),
          },
        ],
        last_seen_ms: Date.now(),
      });
    if (phase === "error") return json({ error: "Disconnected" }, 503);
    if (path.startsWith("chat/"))
      return json(
        phase === "ended"
          ? [
              {
                ...turn,
                request: { ...request, action: { type: "cancel" } },
                state: "completed",
                outcome: {
                  type: "completed",
                  event: { kind: { type: "cancelled" }, artifacts: [] },
                },
              },
            ]
          : phase === "rejected"
            ? [
                {
                  ...turn,
                  state: "completed",
                  outcome: { type: "rejected", reason: "Provider unavailable" },
                },
              ]
            : phase === "empty-reply"
              ? [
                  {
                    ...turn,
                    state: "completed",
                    outcome: {
                      type: "completed",
                      event: { kind: { type: "replied" }, artifacts: [] },
                    },
                  },
                ]
              : [turn],
      );
    return json([]);
  });
  render(
    <QueryClientProvider client={cache}>
      <ChatPage
        config={config}
        active
        onSettings={vi.fn()}
        onLogout={vi.fn()}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("option", { name: "Second · Claude Code" });
  for (const section of ["Routines", "Workflows", "Plugins"]) {
    await user.click(screen.getByRole("button", { name: section }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: section })).toHaveFocus(),
    );
  }
  await user.click(screen.getByRole("button", { name: "Chats" }));
  await waitFor(() =>
    expect(
      screen.getByRole("textbox", { name: "Message Cuttlefish" }),
    ).toHaveFocus(),
  );
  await user.selectOptions(
    screen.getByLabelText("Workspace and provider"),
    "second",
  );
  await user.type(screen.getByLabelText("Search chats"), "none");
  expect(screen.queryByRole("button", { name: "Chat" })).toBeNull();
  await user.clear(screen.getByLabelText("Search chats"));
  await user.click(await screen.findByRole("button", { name: "Chat" }));
  await waitFor(() =>
    expect(document.getElementById("chat-status")).toHaveTextContent(
      "Claude Code is reading files…",
    ),
  );
  const scroll = document.getElementById("conversation-scroll")!;
  Object.defineProperty(scroll, "scrollHeight", {
    value: 1000,
    configurable: true,
  });
  fireEvent.scroll(scroll);
  phase = "rejected";
  await cache.invalidateQueries({ queryKey: ["turns"] });
  expect(await screen.findByText("Provider unavailable")).toBeVisible();
  expect(scroll.scrollTop).toBe(0);
  await user.click(screen.getByRole("button", { name: "Jump to latest" }));
  expect(scroll.scrollTop).toBe(1000);
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  fireEvent.scroll(scroll);
  phase = "empty-reply";
  await cache.invalidateQueries({ queryKey: ["turns"] });
  await waitFor(() =>
    expect(screen.queryByText("Provider unavailable")).toBeNull(),
  );
  await user.click(
    screen.getByRole("button", { name: "Conversation options" }),
  );
  await user.click(
    await screen.findByRole("menuitem", { name: "End session" }),
  );
  await waitFor(() =>
    expect(document.getElementById("chat-status")).toHaveTextContent(
      "Session ended",
    ),
  );
  await user.click(screen.getByRole("button", { name: "New chat" }));
  expect(
    screen.getByRole("heading", { name: "What would you like to do?" }),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Chat" }));
  phase = "error";
  await cache.invalidateQueries({ queryKey: ["turns"] });
  await waitFor(() =>
    expect(document.getElementById("chat-status")).toHaveTextContent(
      "Connection interrupted",
    ),
  );
});
