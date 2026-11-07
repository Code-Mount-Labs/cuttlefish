import { it, expect, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SettingsPage } from "../src/features/settings/settings-page";
import { config, json, workspace } from "./fixtures";
function mount(value = config) {
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <SettingsPage config={value} onLogout={vi.fn()} />
    </QueryClientProvider>,
  );
}
it("checks providers and creates an immutable workspace with an exact retry", async () => {
  const user = userEvent.setup(),
    bodies: unknown[] = [];
  let fail = true,
    checks = 0;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const path = String(input).split("/").at(-1);
    if (path === "providers") {
      checks++;
      if (checks === 1) return json({ error: "Provider unavailable" }, 503);
      return json([
        { provider: "claude", installed: true, signed_in: checks === 2 },
        { provider: "codex", installed: checks > 2, signed_in: false },
      ]);
    }
    if (init?.method === "POST") {
      bodies.push(JSON.parse(String(init.body)));
      if (fail) {
        fail = false;
        return json({ error: "Try workspace again" }, 503);
      }
      return json({});
    }
    return json([
      { workspace, directory: "/workspace/research" },
      {
        workspace: {
          ...workspace,
          workspace_id: "other",
          provider: "claude",
          allow_edits: false,
        },
        directory: "/workspace/other",
      },
    ]);
  });
  mount();
  await user.click(
    screen.getByRole("button", { name: "Check local sign-ins" }),
  );
  expect(await screen.findByText("Provider unavailable")).toBeVisible();
  await user.click(
    screen.getByRole("button", { name: "Check local sign-ins" }),
  );
  expect(await screen.findByText("Claude Code: Signed in")).toBeVisible();
  expect(screen.getByText("Codex: Install the CLI")).toBeVisible();
  await user.click(
    screen.getByRole("button", { name: "Check local sign-ins" }),
  );
  expect(
    await screen.findByText("Claude Code: Sign in on this machine"),
  ).toBeVisible();
  await user.type(screen.getByLabelText("Workspace name"), "Work");
  await user.selectOptions(screen.getByLabelText("Provider"), "claude");
  await user.click(screen.getByLabelText("Allow file edits"));
  await user.click(screen.getByRole("button", { name: "Create workspace" }));
  expect(await screen.findByText("Try workspace again")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Create workspace" }));
  await waitFor(() => expect(bodies).toHaveLength(2));
  expect(bodies[0]).toEqual(bodies[1]);
  await screen.findByText("Saved");
  await user.selectOptions(screen.getByLabelText("Provider"), "codex");
  await user.click(screen.getByRole("button", { name: "Create workspace" }));
  await waitFor(() => expect(bodies).toHaveLength(3));
  expect(bodies[2]).toMatchObject({ provider: "codex" });
});
it("supports managed database provisioning, retry, deferral and remote mode changes", async () => {
  const user = userEvent.setup(),
    bodies: { url: string; body: unknown }[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") {
      bodies.push({ url: String(input), body: JSON.parse(String(init.body)) });
      return json({});
    }
    return json({ error: "Workspaces offline" }, 503);
  });
  mount({
    ...config,
    database: {
      state: "failed",
      mode: "managed",
      error_code: "docker_not_ready",
    },
    remote: {
      revision: 1,
      state: "ready",
      choice: { mode: "managed" },
      public_url: "https://example.org/",
    },
    model: { configured: true, model: "configured-model" },
  });
  expect(
    screen.getByRole("link", { name: "Open remote dashboard" }),
  ).toHaveAttribute("href", "https://example.org/");
  expect(await screen.findByText("Workspaces offline")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Save access" }));
  await user.selectOptions(screen.getByLabelText("Access"), "local");
  await user.click(screen.getByRole("button", { name: "Save access" }));
  await user.click(screen.getByRole("button", { name: "Save database" }));
  await user.click(screen.getByRole("button", { name: "Retry saved setup" }));
  await user.selectOptions(screen.getByLabelText("Database setup"), "later");
  await user.click(screen.getByRole("button", { name: "Save database" }));
  await waitFor(() => expect(bodies).toHaveLength(5));
  expect(bodies.map((x) => x.body)).toEqual([
    { mode: "managed" },
    { mode: "local" },
    { mode: "managed", port: 54329 },
    {},
    { mode: "later" },
  ]);
});
it("keeps configuration failures visible and never displays an unsafe remote link", async () => {
  vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
    json({ error: "Save failed" }, 503),
  );
  mount({
    ...config,
    role: "router",
    remote: {
      ...config.remote,
      state: "ready",
      public_url: "javascript:alert(1)",
    },
    database: { state: "failed", mode: "existing" },
  });
  expect(screen.queryByRole("link")).toBeNull();
  fireEvent.submit(screen.getByLabelText("Access").closest("form")!);
  expect(await screen.findByText("Save failed")).toBeVisible();
});
