import { it, expect, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AutomationPage } from "../src/features/automations/automation-page";
import { workspace, json } from "./fixtures";
import {
  localTime,
  formatTime,
  schedulePreview,
} from "../src/features/automations/model";

const workflow = {
  id: "00000000-0000-0000-0000-000000000101",
  revision: 1,
  name: "Review",
  workspace_id: workspace.workspace_id,
  instructions: "Review this project",
};
const routine = {
  ...workflow,
  id: "00000000-0000-0000-0000-000000000102",
  name: "Morning",
  enabled: true,
  first_run_ms: Date.now() + 3600000,
  interval_minutes: 1440,
  next_run_ms: Date.now() + 3600000,
};
const run = {
  id: "00000000-0000-0000-0000-000000000103",
  workflow_id: workflow.id,
  routine_id: null,
  route_id: null,
  thread_id: "00000000-0000-0000-0000-000000000103",
  created_ms: Date.now(),
  state: "completed",
  error: null,
};
const empty = { workflows: [], routines: [], runs: [] };
function mount(section: "routines" | "workflows", open = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AutomationPage section={section} onOpenChat={open} />
    </QueryClientProvider>,
  );
}
it("creates and edits reusable prompts, safely retries a run, and opens its conversation", async () => {
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
      ? json([{ workspace }])
      : json({
          ...empty,
          workflows: saved ? [workflow] : [],
          runs: saved ? [run] : [],
        });
  });
  mount("workflows", open);
  expect(await screen.findByText("No workflows yet")).toBeVisible();
  expect(
    screen.getByText("Save a prompt you want to run again."),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "New workflow" }));
  await user.type(screen.getByLabelText("Name"), " Review ");
  await user.type(screen.getByLabelText("Prompt"), " Review this project ");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(
    await screen.findByRole("button", { name: "Edit Review" }),
  ).toBeVisible();
  expect(bodies[0]).toMatchObject({
    type: "save_workflow",
    workflow: { name: "Review", instructions: "Review this project" },
  });
  await user.click(screen.getByRole("button", { name: "Edit Review" }));
  expect(screen.getByLabelText("Prompt")).toHaveValue(workflow.instructions);
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Run Review" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Connection lost");
  await user.click(screen.getByRole("button", { name: "Run Review" }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  const runs = bodies.filter((body) => body.type === "run");
  expect(runs).toHaveLength(2);
  expect(runs[0]).toEqual(runs[1]);
  expect(screen.queryByText(/queued\. Open/)).toBeNull();
  await user.click(
    screen.getByRole("button", { name: "Open Review conversation" }),
  );
  expect(open).toHaveBeenCalledWith(run.thread_id);
});
it("creates a routine directly from a prompt without any workflow, runs it, and pauses and resumes it", async () => {
  const user = userEvent.setup();
  let saved = false,
    enabled = true;
  const bodies: Record<string, unknown>[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      bodies.push(body);
      if (body.routine) enabled = body.routine.enabled;
      saved = true;
      return json({ ok: true });
    }
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({
          ...empty,
          routines: saved ? [{ ...routine, enabled }] : [],
          runs: [
            {
              ...run,
              workflow_id: null,
              routine_id: routine.id,
              state: "pending",
              error: "Waiting for an available agent",
            },
          ],
        });
  });
  mount("routines");
  expect(
    await screen.findByText("Give an agent a prompt and choose when it runs."),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "New routine" }));
  expect(screen.queryByLabelText("Workflow")).toBeNull();
  await user.type(screen.getByLabelText("Name"), "Morning");
  await user.type(screen.getByLabelText("Prompt"), "Summarize the news");
  await user.selectOptions(screen.getByLabelText("Repeat"), "60");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(
    await screen.findByRole("button", { name: "Pause Morning" }),
  ).toBeVisible();
  expect(bodies[0]).toMatchObject({
    type: "save_routine",
    routine: {
      name: "Morning",
      instructions: "Summarize the news",
      workspace_id: workspace.workspace_id,
      interval_minutes: 60,
    },
  });
  expect(
    (bodies[0].routine as Record<string, unknown>).next_run_ms,
  ).toBeUndefined();
  await user.click(screen.getByRole("button", { name: "Edit Morning" }));
  expect(screen.getByLabelText("Prompt")).toHaveValue(routine.instructions);
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Run Morning" }));
  await waitFor(() =>
    expect(bodies.some((body) => body.type === "run_routine")).toBe(true),
  );
  await user.click(screen.getByRole("button", { name: "Pause Morning" }));
  expect(await screen.findByText("Paused")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Resume Morning" }));
  expect(await screen.findByText("Active")).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Open Morning conversation" }),
  ).toBeDisabled();
  expect(screen.getByText("Queued")).toBeVisible();
  expect(screen.getByText("Waiting for an available agent")).toBeVisible();
});
it("shows an explicit update requirement for old servers without blocking legacy workflows", async () => {
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({
          workflows: [workflow],
          routes: [],
          runs: [{ ...run, routine_id: undefined }],
        }),
  );
  const view = mount("routines");
  expect(await screen.findByText("Update required")).toBeVisible();
  expect(screen.getByRole("button", { name: "New routine" })).toBeDisabled();
  view.unmount();
  mount("workflows");
  expect(
    await screen.findByRole("button", { name: "Run Review" }),
  ).toBeEnabled();
});
it("handles missing workspaces and permits inspecting an existing definition", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces")
      ? json([])
      : json({
          ...empty,
          workflows: [workflow],
          runs: [{ ...run, workflow_id: "missing" }],
        }),
  );
  mount("workflows");
  expect(await screen.findByText("Workspace unavailable")).toBeVisible();
  expect(screen.getByRole("button", { name: "Run Review" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "New workflow" })).toBeDisabled();
  expect(screen.getByText("Workflow")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Edit Review" }));
  expect(screen.getByLabelText("Agent workspace")).toHaveProperty("value", "");
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});
it("explains missing workspace setup in an empty state", async () => {
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces") ? json([]) : json(empty),
  );
  mount("routines");
  expect(
    await screen.findByText(
      "Add an agent workspace in Settings to get started.",
    ),
  ).toBeVisible();
});
it.each(["automations", "workspaces"])(
  "reports %s read failures",
  async (failed) => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
      String(input).endsWith(failed)
        ? json({ error: "Read unavailable" }, 503)
        : String(input).endsWith("workspaces")
          ? json([])
          : json(empty),
    );
    mount("workflows");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Read unavailable",
    );
  },
);
it("keeps a pending save open and exposes its failure for retry", async () => {
  const user = userEvent.setup();
  let resolve!: (value: Response) => void;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST")
      return new Promise<Response>((done) => {
        resolve = done;
      });
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, workflows: [workflow] });
  });
  mount("workflows");
  await user.click(await screen.findByRole("button", { name: "Edit Review" }));
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByRole("button", { name: "Saving…" })).toBeDisabled();
  await user.keyboard("{Escape}");
  expect(screen.getByRole("dialog")).toBeVisible();
  resolve(json({ error: "Save failed" }, 503));
  expect(await screen.findByRole("alert")).toHaveTextContent("Save failed");
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});
it("validates whitespace-only prompts and invalid schedule values before submission", async () => {
  const user = userEvent.setup();
  const post = vi.fn();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") post();
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, routines: [routine] });
  });
  mount("routines");
  await user.click(await screen.findByRole("button", { name: "Edit Morning" }));
  await user.clear(screen.getByLabelText("Prompt"));
  await user.type(screen.getByLabelText("Prompt"), "   ");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Enter a name");
  await user.type(screen.getByLabelText("Prompt"), "Run task");
  await user.clear(screen.getByLabelText("Name"));
  await user.type(screen.getByLabelText("Name"), " ");
  fireEvent.submit(
    screen.getByRole("button", { name: "Save" }).closest("form")!,
  );
  await user.type(screen.getByLabelText("Name"), "Morning");
  fireEvent.change(screen.getByLabelText("First run"), {
    target: { value: "" },
  });
  fireEvent.submit(
    screen.getByRole("button", { name: "Save" }).closest("form")!,
  );
  expect(post).not.toHaveBeenCalled();
});
it("preserves custom intervals, translates execution states, and handles a failed pause", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) =>
    init?.method === "POST"
      ? json({ error: "Could not pause" }, 503)
      : String(input).endsWith("workspaces")
        ? json([{ workspace }])
        : json({
            ...empty,
            routines: [{ ...routine, interval_minutes: 17 }],
            runs: [
              "claimed",
              "queued",
              "completed",
              "failed",
              "cancelled",
              "unknown",
            ].map((state, index) => ({
              ...run,
              id: String(index),
              workflow_id: null,
              routine_id: index === 0 ? null : routine.id,
              route_id: index === 0 ? "old-route" : null,
              state,
            })),
          }),
  );
  mount("routines");
  expect(await screen.findByText("Every 17 minutes")).toBeVisible();
  expect(screen.getByText("Running")).toBeVisible();
  expect(screen.getByText("Awaiting an update")).toBeVisible();
  expect(screen.getByText("Stopped")).toBeVisible();
  expect(screen.getByText("Routine")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Edit Morning" }));
  expect(screen.getByLabelText("Repeat")).toHaveValue("17");
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  await user.click(screen.getByRole("button", { name: "Pause Morning" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not pause");
});
it("retains a useful error when browser storage prevents a safe run identity", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, workflows: [workflow] }),
  );
  mount("workflows");
  await screen.findByRole("button", { name: "Run Review" });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("Storage unavailable");
  });
  await user.click(screen.getByRole("button", { name: "Run Review" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Storage unavailable",
  );
});

it("shows the pending resume state and locks other actions until the change finishes", async () => {
  const user = userEvent.setup();
  let resolve!: (value: Response) => void;
  let enabled = false;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST")
      return new Promise<Response>((done) => {
        resolve = done;
      });
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, routines: [{ ...routine, enabled }] });
  });
  mount("routines");
  await user.click(
    await screen.findByRole("button", { name: "Resume Morning" }),
  );
  expect(
    await screen.findByRole("button", { name: "Resuming… Morning" }),
  ).toBeDisabled();
  expect(screen.getByRole("button", { name: "Edit Morning" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Run Morning" })).toBeDisabled();
  enabled = true;
  resolve(json({ ok: true }));
  expect(
    await screen.findByRole("button", { name: "Pause Morning" }),
  ).toBeEnabled();
});

it("previews schedule changes and preserves exact first-run time when only the prompt changes", async () => {
  const user = userEvent.setup();
  const first = new Date(2030, 0, 2, 9, 30, 42, 123).getTime();
  const next = first + 86400000;
  const scheduled = { ...routine, first_run_ms: first, next_run_ms: next };
  const bodies: unknown[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    if (init?.method === "POST") {
      bodies.push(JSON.parse(String(init.body)));
      return json({ ok: true });
    }
    return String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, routines: [scheduled] });
  });
  mount("routines");
  await user.click(await screen.findByRole("button", { name: "Edit Morning" }));
  expect(screen.getByRole("status")).toHaveTextContent(
    `Next run ${formatTime(next)}.`,
  );
  await user.type(screen.getByLabelText("Prompt"), " and summarize");
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(bodies[0]).toMatchObject({
    routine: {
      first_run_ms: first,
      instructions: "Review this project and summarize",
    },
  });
  await user.click(screen.getByRole("button", { name: "Edit Morning" }));
  fireEvent.change(screen.getByLabelText("First run"), {
    target: { value: "2020-01-01T09:30" },
  });
  expect(screen.getByRole("status")).toHaveTextContent(
    "Due now. Runs when the router is available; missed runs are combined.",
  );
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(bodies).toHaveLength(2));
  expect(bodies[1]).toMatchObject({
    routine: { first_run_ms: new Date("2020-01-01T09:30").getTime() },
  });
});

it("explains paused, invalid, preserved and changed schedules without inventing an immediate run", () => {
  const first = new Date(2030, 0, 2, 9, 30, 42, 123).getTime();
  const next = first + 86400000;
  const scheduled = { ...routine, first_run_ms: first, next_run_ms: next };
  const unchanged = localTime(first);
  expect(schedulePreview(scheduled, unchanged, 1440, first + 1)).toBe(
    `Next run ${formatTime(next)}.`,
  );
  expect(schedulePreview(scheduled, unchanged, 60, first + 1)).toBe(
    "Due now. Runs when the router is available; missed runs are combined.",
  );
  expect(
    schedulePreview({ ...scheduled, enabled: false }, unchanged, 1440, next),
  ).toBe("Paused. This routine will not run until resumed.");
  expect(schedulePreview(scheduled, "", 1440, first)).toBe(
    "Choose a valid first-run time.",
  );
  expect(schedulePreview(scheduled, localTime(next), 1440, first)).toBe(
    `Next run ${formatTime(next)}.`,
  );
});

it("keeps workflow validation focused on its name and prompt", async () => {
  const user = userEvent.setup();
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) =>
    String(input).endsWith("workspaces")
      ? json([{ workspace }])
      : json({ ...empty, workflows: [workflow] }),
  );
  mount("workflows");
  await user.click(await screen.findByRole("button", { name: "Edit Review" }));
  expect(screen.getByLabelText("Prompt")).toHaveClass("min-h-28");
  await user.clear(screen.getByLabelText("Prompt"));
  await user.type(screen.getByLabelText("Prompt"), " ");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Enter a name and a prompt.",
  );
});
