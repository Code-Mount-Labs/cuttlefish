import { it, expect, vi } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { UpdatesPanel } from "../src/features/updates/updates-panel";
import { json } from "./fixtures";
const initial = {
  current_version: "v0.2.0-alpha.6",
  available_version: null as string | null,
  checked: false,
  check_error: null as string | null,
  supported: true,
  automatic_restart: true,
  job: null as null | {
    id: string;
    version: string;
    phase: string;
    restart_only: boolean;
    error: string | null;
  },
};
function mount(enabled = true) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <UpdatesPanel enabled={enabled} />
    </QueryClientProvider>,
  );
  return cache;
}
it("does not query older servers and explains unmanaged installations", async () => {
  const fetch = vi
    .spyOn(globalThis, "fetch")
    .mockResolvedValue(json({ ...initial, supported: false }));
  mount(false);
  expect(
    screen.getByText(/Install the latest Cuttlefish release once/),
  ).toBeVisible();
  expect(fetch).not.toHaveBeenCalled();
  cleanup();
  mount();
  expect(await screen.findByText(/Use the Cuttlefish installer/)).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "Check for updates" }),
  ).not.toBeInTheDocument();
});
it("checks a release, installs it, survives a lost response, and resumes progress", async () => {
  const user = userEvent.setup();
  let state = { ...initial };
  const actions: unknown[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST") {
      const action = JSON.parse(String(init.body));
      actions.push(action);
      if (action.action === "check")
        state = {
          ...state,
          checked: true,
          available_version: "v0.2.0-alpha.7",
        };
      if (action.action === "install") {
        state = {
          ...state,
          job: {
            id: "update",
            version: "v0.2.0-alpha.7",
            phase: "downloading",
            restart_only: false,
            error: null,
          },
        };
        throw new TypeError("Connection interrupted");
      }
    }
    return json(state);
  });
  const cache = mount();
  await user.click(
    await screen.findByRole("button", { name: "Check for updates" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Update and restart" }),
  );
  expect(
    await screen.findByText("Downloading and verifying the update…"),
  ).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Check for updates" }),
  ).toBeDisabled();
  state = { ...state, job: { ...state.job!, phase: "restarting" } };
  await cache.invalidateQueries({ queryKey: ["updates"] });
  expect(
    await screen.findByText("Restarting Cuttlefish. Reconnecting…"),
  ).toBeVisible();
  state = {
    ...state,
    current_version: "v0.2.0-alpha.7",
    available_version: null,
    job: { ...state.job!, phase: "complete" },
  };
  await cache.invalidateQueries({ queryKey: ["updates"] });
  expect(
    await screen.findByRole("button", {
      name: "Reload to use the new version",
    }),
  ).toBeEnabled();
  expect(actions).toEqual([
    { action: "check" },
    { action: "install", version: "v0.2.0-alpha.7" },
  ]);
});
it("restarts the service without installation and retries failures", async () => {
  const user = userEvent.setup();
  let state = { ...initial };
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST") {
      const action = JSON.parse(String(init.body));
      state = {
        ...state,
        job: {
          id: "restart",
          version: initial.current_version,
          phase: action.action === "retry" ? "complete" : "failed",
          restart_only: true,
          error: action.action === "retry" ? null : "Restart unavailable",
        },
      };
    }
    return json(state);
  });
  mount();
  await user.click(
    await screen.findByRole("button", { name: "Restart Cuttlefish" }),
  );
  expect(await screen.findByText("Restart unavailable")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Retry" }));
  expect(
    await screen.findByText("Ready. Your chats and settings are unchanged."),
  ).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "Reload to use the new version" }),
  ).not.toBeInTheDocument();
});
it("supports foreground installation and shows release check errors", async () => {
  const user = userEvent.setup();
  let state = {
    ...initial,
    automatic_restart: false,
    available_version: "v0.2.0-alpha.7",
    check_error: "Release check unavailable",
  };
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST")
      state = {
        ...state,
        check_error: "",
        job: {
          id: "install",
          version: "v0.2.0-alpha.7",
          phase: "restart_required",
          restart_only: false,
          error: null,
        },
      };
    return json(state);
  });
  mount();
  expect(await screen.findByText("Release check unavailable")).toBeVisible();
  expect(screen.getByText(/Background startup is not enabled/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Install update" }));
  expect(
    await screen.findByText(/Update installed. Restart your foreground/),
  ).toBeVisible();
});
it("recovers a failed status request and confirms the current version", async () => {
  const user = userEvent.setup();
  let calls = 0;
  vi.spyOn(globalThis, "fetch").mockImplementation(async () => {
    if (calls++ === 0) throw new TypeError("Offline");
    return json({ ...initial, checked: true });
  });
  mount();
  expect(await screen.findByText("Offline")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() =>
    expect(screen.getByText("You’re up to date.")).toBeVisible(),
  );
});

it("keeps pending actions explicit and reloads only when requested", async () => {
  const user = userEvent.setup();
  let resolve: (value: Response) => void = () => {};
  let state = { ...initial };
  vi.spyOn(globalThis, "fetch").mockImplementation(async (_, init) => {
    if (init?.method === "POST")
      return new Promise<Response>((done) => {
        resolve = done;
      });
    return json(state);
  });
  mount();
  await user.click(
    await screen.findByRole("button", { name: "Check for updates" }),
  );
  expect(
    await screen.findByRole("button", { name: "Checking…" }),
  ).toBeDisabled();
  state = {
    ...state,
    job: {
      id: "ready",
      version: "v0.2.0-alpha.7",
      restart_only: false,
      phase: "complete",
      error: null,
    },
  };
  resolve(json(state));
  const reload = vi.fn();
  const original = globalThis.location;
  vi.stubGlobal("location", { ...original, reload });
  await user.click(
    await screen.findByRole("button", {
      name: "Reload to use the new version",
    }),
  );
  expect(reload).toHaveBeenCalledOnce();
  vi.stubGlobal("location", original);
  await user.click(screen.getByRole("button", { name: "Restart Cuttlefish" }));
  expect(
    screen.getByRole("button", { name: "Check for updates" }),
  ).toBeDisabled();
  resolve(json(state));
});

it("keeps saved progress visible during a service reconnect", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    json({
      ...initial,
      job: {
        id: "queued",
        version: "v0.2.0-alpha.7",
        phase: "queued",
        restart_only: false,
        error: null,
      },
    }),
  );
  mount();
  expect(await screen.findByText("Preparing…")).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Check for updates" }),
  ).toBeDisabled();
});
