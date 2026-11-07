import { it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
