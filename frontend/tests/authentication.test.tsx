import { it, expect, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  focusManager,
  onlineManager,
  QueryClient,
  QueryClientProvider,
} from "@tanstack/react-query";
import { LocalApp } from "../src/app/local-app";
import { PlatformApp } from "../src/app/platform-app";
import { config, json } from "./fixtures";
function mount(local = true) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      {local ? <LocalApp /> : <PlatformApp />}
    </QueryClientProvider>,
  );
  return cache;
}
async function credentials(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("Username"), "operator");
  await user.type(screen.getByLabelText("Password"), "long-password");
}
it.each([true, false])(
  "local authentication and logout (initialized=%s)",
  async (initialized) => {
    let signedIn = false,
      ready = initialized,
      failLogout = true;
    const user = userEvent.setup();
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input) => {
        const path = String(input).replace("/api/local/", "");
        if (path === "status") return json({ initialized: ready });
        if (path === "login" || path === "setup") {
          signedIn = true;
          ready = true;
          return json({});
        }
        if (path === "config")
          return signedIn
            ? json({ ...config, role: initialized ? "router_agent" : "agent" })
            : json({ error: "unauthorized" }, 401);
        if (path === "logout") {
          if (failLogout) {
            failLogout = false;
            return json({ error: "Sign out failed" }, 503);
          }
          signedIn = false;
          return json({});
        }
        if (
          path === "chat" ||
          path === "workspaces" ||
          path === "chat/workspaces"
        )
          return json([]);
        return json({});
      });
    mount();
    await screen.findByRole("heading", {
      name: initialized ? "Sign in" : "Set up this machine",
    });
    if (!initialized)
      await user.type(screen.getByLabelText("Setup code"), "setup");
    await credentials(user);
    await user.click(
      screen.getByRole("button", {
        name: initialized ? "Sign in" : "Create administrator",
      }),
    );
    await screen.findByRole("button", { name: "Sign out" });
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(await screen.findByText("Sign out failed")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeVisible();
    expect(
      fetch.mock.calls.some(([url]) =>
        String(url).endsWith(initialized ? "/login" : "/setup"),
      ),
    ).toBe(true);
  },
);
it.each(["status", "config"])(
  "local %s failure can be retried",
  async (broken) => {
    let fail = true;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const path = String(input).split("/").at(-1);
      if (path === broken && fail)
        return json({ error: "Connection unavailable" }, 503);
      if (path === "status") return json({ initialized: true });
      if (path === "config")
        return json({ ...config, role: "router", features: undefined });
      return json([]);
    });
    mount();
    expect(await screen.findByText("Connection unavailable")).toBeVisible();
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeVisible();
  },
);
it.each([false, true])(
  "platform sign-in/register clears old data (register=%s)",
  async (register) => {
    const user = userEvent.setup();
    let signedIn = false,
      failLogout = true;
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input) => {
        const path = String(input);
        if (path.endsWith("/session"))
          return signedIn
            ? json({ username: "operator" })
            : json({ error: "unauthorized" }, 401);
        if (path.endsWith("/register") || path.endsWith("/login")) {
          signedIn = true;
          return json({});
        }
        if (path.endsWith("/logout")) {
          if (failLogout) {
            failLogout = false;
            return json({ error: "Try sign out again" }, 503);
          }
          signedIn = false;
          return json({});
        }
        return json([]);
      });
    const cache = mount(false);
    await screen.findByRole("heading", { name: "Sign in" });
    cache.setQueryData(["private-data"], ["old"]);
    if (register) {
      await user.click(screen.getByRole("button", { name: "Create account" }));
      await user.click(screen.getByRole("button", { name: "Sign in instead" }));
      await user.click(screen.getByRole("button", { name: "Create account" }));
    }
    await credentials(user);
    await user.click(
      screen.getByRole("button", {
        name: register ? "Create account" : "Sign in",
      }),
    );
    await screen.findByRole("heading", { name: "Groups" });
    expect(cache.getQueryData(["private-data"])).toBeUndefined();
    await user.click(screen.getByRole("button", { name: "Installation" }));
    await user.click(screen.getByRole("button", { name: "Groups" }));
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(await screen.findByText("Try sign out again")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    await waitFor(() =>
      expect(screen.queryByRole("heading", { name: "Groups" })).toBeNull(),
    );
    expect(
      fetch.mock.calls.some(([url]) =>
        String(url).endsWith(register ? "/register" : "/login"),
      ),
    ).toBe(true);
  },
);
it("platform connection errors have a working retry", async () => {
  let fail = true;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("/session")
      ? fail
        ? json({ error: "Offline" }, 503)
        : json({})
      : json([]),
  );
  mount(false);
  expect(await screen.findByText("Offline")).toBeVisible();
  fail = false;
  await userEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("heading", { name: "Groups" })).toBeVisible();
});

it.each(["login", "setup", "platform", "register"])(
  "%s keeps input and focus through background session checks and connection errors",
  async (mode) => {
    const local = mode === "login" || mode === "setup";
    let pending: ((response: Response) => void) | undefined;
    let delay = false;
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input) => {
        if (String(input).endsWith("/status"))
          return json({ initialized: mode !== "setup" });
        if (delay)
          return new Promise<Response>((resolve) => {
            pending = resolve;
          });
        return json({ error: "unauthorized" }, 401);
      });
    const cache = mount(local);
    const user = userEvent.setup();
    await screen.findByLabelText("Username");
    if (mode === "register")
      await user.click(screen.getByRole("button", { name: "Create account" }));
    if (mode === "setup")
      await user.type(screen.getByLabelText("Setup code"), "my-code");
    await credentials(user);
    const password = screen.getByLabelText("Password");
    expect(password).toHaveFocus();
    delay = true;
    for (const status of [401, 503]) {
      let refresh: Promise<void>;
      act(() => {
        refresh = cache.invalidateQueries({
          queryKey: [local ? "config" : "session"],
        });
      });
      await waitFor(() => expect(pending).toBeTypeOf("function"));
      expect(screen.getByLabelText("Password")).toBe(password);
      expect(password).toHaveValue("long-password");
      expect(password).toHaveFocus();
      await act(async () => {
        pending!(json({ error: "temporary" }, status));
        await refresh!;
      });
      pending = undefined;
      expect(screen.getByLabelText("Password")).toBe(password);
      expect(password).toHaveFocus();
      expect(screen.getByLabelText("Username")).toHaveValue("operator");
      if (mode === "setup")
        expect(screen.getByLabelText("Setup code")).toHaveValue("my-code");
    }
    const before = fetch.mock.calls.length;
    await act(async () => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
      onlineManager.setOnline(false);
      onlineManager.setOnline(true);
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    expect(fetch.mock.calls.length).toBe(before + (local ? 1 : 0));
    expect(screen.getByLabelText("Password")).toBe(password);
    expect(password).toHaveValue("long-password");
  },
);

it("a background platform outage preserves the account form", async () => {
  let fail = false;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("/session")
      ? fail
        ? json({ error: "Connection interrupted" }, 503)
        : json({ username: "operator" })
      : json([]),
  );
  const cache = mount(false);
  await userEvent.click(await screen.findByRole("button", { name: "Account" }));
  const field = screen.getByLabelText("Current password");
  await userEvent.type(field, "unsaved-password");
  fail = true;
  await act(async () => {
    await cache.invalidateQueries({ queryKey: ["session"] });
  });
  expect(await screen.findByText("Connection interrupted")).toBeVisible();
  expect(screen.getByLabelText("Current password")).toBe(field);
  expect(field).toHaveValue("unsaved-password");
  expect(field).toHaveFocus();
});

it("background provisioning and relay revisions preserve unsaved setup fields", async () => {
  let current = {
    ...config,
    database: { ...config.database, state: "unconfigured" },
  };
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const path = String(input).split("/").at(-1);
    if (path === "status") return json({ initialized: true });
    if (path === "config") return json(current);
    return json([]);
  });
  const cache = mount();
  const user = userEvent.setup();
  await screen.findByRole("heading", { name: "Settings" });
  await user.selectOptions(screen.getByLabelText("Access"), "existing");
  const address = screen.getByLabelText("HTTPS address");
  await user.clear(address);
  await user.type(address, "https://draft.example");
  current = {
    ...current,
    database: { ...current.database, state: "ready" },
    remote: { ...current.remote, revision: current.remote.revision + 1 },
  };
  await act(async () => {
    await cache.invalidateQueries({ queryKey: ["config"] });
  });
  expect(screen.getByRole("heading", { name: "Settings" })).toBeVisible();
  expect(screen.getByLabelText("HTTPS address")).toBe(address);
  expect(address).toHaveValue("https://draft.example");
  expect(address).toHaveFocus();
  await user.click(await screen.findByRole("button", { name: "Chats" }));
  expect(screen.getByLabelText("Message Cuttlefish")).toBeVisible();
});

it.each([true, false])(
  "signed-in sessions still revalidate on focus and reconnect (local=%s)",
  async (local) => {
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input) => {
        const path = String(input).split("/").at(-1);
        if (path === "status") return json({ initialized: true });
        if (path === "config") return json(config);
        if (path === "session") return json({ username: "operator" });
        return json([]);
      });
    mount(local);
    await screen.findByRole("button", { name: "Sign out" });
    const count = () =>
      fetch.mock.calls.filter(([url]) =>
        String(url).endsWith(local ? "/config" : "/session"),
      ).length;
    const before = count();
    await act(async () => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => expect(count()).toBeGreaterThan(before));
    const afterFocus = count();
    await act(async () => {
      onlineManager.setOnline(false);
      onlineManager.setOnline(true);
    });
    await waitFor(() => expect(count()).toBeGreaterThan(afterFocus));
  },
);
