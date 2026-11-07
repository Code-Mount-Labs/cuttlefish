import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Message } from "../src/features/chat/message";
import { TaskActivity } from "../src/features/chat/activity";
import { AuthForm } from "../src/features/auth/auth-form";
import { Composer } from "../src/features/chat/composer";
import { LocalApp } from "../src/app/local-app";
import { PlatformApp } from "../src/app/platform-app";
import { SettingsPage } from "../src/features/settings/settings-page";
import { config, json, request, turn, workspace } from "./fixtures";
function mount(component: ReactNode) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={cache}>{component}</QueryClientProvider>,
  );
}
describe("semantic chat components", () => {
  it("renders safe rich text, tables, and copy feedback", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
    mount(
      <Message
        speaker="agent"
        text={
          "# Result\n\n**Ready** [source](https://example.com) [unsafe](javascript:alert)\n\n<script>alert(1)</script>\n\n| A | B |\n| - | - |\n| one | two |"
        }
      />,
    );
    expect(screen.getByRole("heading", { name: "Result" })).toBeVisible();
    expect(screen.getByRole("table")).toBeVisible();
    expect(screen.getByRole("link", { name: "source" })).toHaveAttribute(
      "rel",
      "noopener noreferrer",
    );
    expect(document.querySelector('script,a[href^="javascript:"]')).toBeNull();
    await user.click(screen.getByRole("button", { name: "Copy response" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeVisible();
  });
  it("shows durable activity details without inventing progress", async () => {
    const user = userEvent.setup();
    mount(
      <TaskActivity
        turn={turn}
        events={[
          {
            sequence: 1,
            command_id: request.command_id,
            provider: "claude",
            phase: "reading_files",
            at_ms: 1000,
          },
        ]}
        lastSeen={Date.now()}
      />,
    );
    expect(screen.getByText("Claude Code is reading files…")).toBeVisible();
    await user.click(screen.getByText("Claude Code is reading files…"));
    expect(screen.getByText("Claude Code is reading files")).toBeVisible();
    expect(document.querySelector("time")).toHaveAttribute(
      "datetime",
      new Date(1000).toISOString(),
    );
  });
  it("supports attachment removal, keyboard send, and locked retry", async () => {
    const user = userEvent.setup(),
      send = vi.fn(),
      files = vi.fn(),
      remove = vi.fn(),
      text = vi.fn();
    const { rerender } = mount(
      <Composer
        text="hello"
        onText={text}
        files={[{ id: "f", file: new File(["x"], "a.txt") }]}
        onFiles={files}
        onRemove={remove}
        onSend={send}
        locked={false}
        disabled={false}
        pending={false}
        status=""
        attachments
      />,
    );
    await user.click(screen.getByRole("button", { name: "Remove a.txt" }));
    expect(remove).toHaveBeenCalledWith("f");
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    expect(send).toHaveBeenCalled();
    fireEvent.paste(screen.getByRole("textbox"), {
      clipboardData: { files: [new File(["a"], "a.txt")] },
    });
    expect(files).toHaveBeenCalled();
    rerender(
      <Composer
        text="hello"
        onText={text}
        files={[]}
        onFiles={files}
        onRemove={remove}
        onSend={send}
        locked
        disabled={false}
        pending
        status=""
        attachments
      />,
    );
    expect(screen.getByRole("textbox")).toHaveAttribute("readonly");
    expect(
      screen.getByRole("button", { name: "Retry same request" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Attach files" })).toBeDisabled();
  });
  it("submits credentials and surfaces failures", async () => {
    const user = userEvent.setup(),
      submit = vi.fn().mockRejectedValue(new Error("Wrong password"));
    mount(<AuthForm setup onSubmit={submit} />);
    await user.type(screen.getByLabelText("Setup code"), "setup");
    await user.type(screen.getByLabelText("Username"), "operator");
    await user.type(screen.getByLabelText("Password"), "long-password");
    await user.click(
      screen.getByRole("button", { name: "Create administrator" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Wrong password",
    );
    expect(submit.mock.calls[0][0]).toEqual({
      username: "operator",
      password: "long-password",
      token: "setup",
    });
  });
});
describe("application workflows", () => {
  it("uses React state for the complete chat and settings flow", async () => {
    const user = userEvent.setup();
    let accepted: typeof request | null = null;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const path = String(input).replace("/api/local/", "");
      if (path === "status") return json({ initialized: true });
      if (path === "config") return json(config);
      if (path === "chat/workspaces") return json([{ workspace }]);
      if (path === "chat" && init?.method === "POST") {
        accepted = JSON.parse(String(init.body));
        return json({ ...turn, request: accepted });
      }
      if (path === "chat")
        return json(
          accepted
            ? [
                {
                  thread_id: accepted.thread_id,
                  workspace_id: workspace.workspace_id,
                  title: "Test message",
                  request: accepted,
                },
              ]
            : [],
        );
      if (path.endsWith("/activity"))
        return json({ events: [], last_seen_ms: Date.now() });
      if (path.startsWith("chat/"))
        return json(
          accepted
            ? [
                {
                  ...turn,
                  request: accepted,
                  state: "completed",
                  outcome: {
                    type: "completed",
                    event: {
                      kind: { type: "replied", text: "Task done" },
                      artifacts: [],
                    },
                  },
                },
              ]
            : [],
        );
      if (path === "workspaces") return json([]);
      return json({});
    });
    mount(<LocalApp />);
    await screen.findByRole("heading", { name: "New chat" });
    await user.type(
      screen.getByRole("textbox", { name: "Message Cuttlefish" }),
      "Test message",
    );
    await user.click(screen.getByRole("button", { name: "Send message" }));
    expect(await screen.findByText("Task done")).toBeVisible();
    expect(screen.getByLabelText("Workspace and provider")).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "Providers and workspaces" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Chats" }));
    expect(screen.getByText("Task done")).toBeVisible();
  });
  it("shows sign-in when the session expires and never displays cached chats", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
      String(input).endsWith("/status")
        ? json({ initialized: true })
        : json({ error: "unauthorized" }, 401),
    );
    mount(<LocalApp />);
    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("textbox", { name: "Message Cuttlefish" }),
    ).toBeNull();
  });
  it("saves remote, database, and model settings through JSON endpoints", async () => {
    const user = userEvent.setup(),
      calls: { url: string; body: unknown }[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      if (init?.body)
        calls.push({ url: String(input), body: JSON.parse(String(init.body)) });
      return json(String(input).endsWith("/workspaces") ? [] : {});
    });
    mount(
      <SettingsPage
        config={{ ...config, database: { state: "unconfigured" } }}
        onLogout={vi.fn()}
      />,
    );
    await user.selectOptions(screen.getByLabelText("Access"), "existing");
    await user.type(
      screen.getByLabelText("HTTPS address"),
      "https://my.example",
    );
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() =>
      expect(calls.some((c) => c.url.endsWith("/remote"))).toBe(true),
    );
    await user.selectOptions(
      screen.getByLabelText("Database setup"),
      "existing",
    );
    await user.type(
      screen.getByLabelText("PostgreSQL connection URL"),
      "postgresql://localhost/db",
    );
    await user.click(screen.getByRole("button", { name: "Save database" }));
    await user.type(screen.getByLabelText("Model"), "model");
    await user.type(screen.getByLabelText("API key"), "secret");
    await user.click(screen.getByRole("button", { name: "Save model" }));
    await waitFor(() => expect(calls).toHaveLength(3));
    expect(screen.getByLabelText("API key")).toHaveValue("");
    expect(calls[1].body).toEqual({
      mode: "existing",
      url: "postgresql://localhost/db",
    });
  });
  it("lists groups, selects machines, and supports an accessible creation dialog", async () => {
    const user = userEvent.setup();
    let created = false;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const path = String(input);
      if (path.endsWith("/session")) return json({ username: "operator" });
      if (path === "/api/v1/groups" && init?.method === "POST") {
        created = true;
        return json({ group_id: "new" });
      }
      if (path.includes("/installations")) return json([]);
      if (path.includes("/groups?"))
        return json(
          created
            ? [{ group_id: "new", name: "Team", agents: 0, online: 0 }]
            : [],
        );
      return json({});
    });
    mount(<PlatformApp />);
    await screen.findByRole("heading", { name: "Groups" });
    await user.click(screen.getByRole("button", { name: "Create group" }));
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.type(screen.getByLabelText("Group name"), "Team");
    fireEvent.submit(screen.getByLabelText("Group name").closest("form")!);
    expect(await screen.findByRole("heading", { name: "Team" })).toBeVisible();
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Installation" }));
    expect(
      screen.getByRole("heading", { name: "Router + agent" }),
    ).toBeVisible();
  });
});
