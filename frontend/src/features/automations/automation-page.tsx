import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Plus,
  Play,
  Pause,
  Pencil,
  Clock3,
  ArrowUpRight,
  Repeat2,
  Workflow as WorkflowIcon,
} from "lucide-react";
import { z } from "@/lib/schema";
import { local, errorMessage } from "@/lib/api";
import { workspaceRowSchema } from "@/lib/contracts";
import { providerLabel } from "@/lib/activity";
import { Button } from "@/components/ui/button";
import { AutomationEditor } from "./automation-editor";
import {
  automationSchema,
  formatTime,
  intervalLabel,
  routineDefinition,
  runStatus,
  type Editor,
  type Routine,
  type Section,
  type Workflow,
} from "./model";

export function AutomationPage({
  section,
  onOpenChat,
}: {
  section: Section;
  onOpenChat: (id: string) => void;
}) {
  const cache = useQueryClient();
  const [editor, setEditor] = useState<Editor | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const [operationError, setOperationError] = useState("");
  const [pendingItem, setPendingItem] = useState("");
  const data = useQuery({
    queryKey: ["automations"],
    queryFn: ({ signal }) => local("automations", automationSchema, { signal }),
    refetchInterval: 5000,
  });
  const workspaces = useQuery({
    queryKey: ["workspaces"],
    queryFn: ({ signal }) =>
      local("chat/workspaces", z.array(workspaceRowSchema), { signal }),
  });
  const change = useMutation({
    mutationFn: (body: unknown) =>
      local("automations", z.object({ ok: z.boolean() }), { body }),
    onSettled: () => cache.invalidateQueries({ queryKey: ["automations"] }),
  });
  const routines = section === "routines";
  const unsupported = routines && !!data.data && !data.data.routines;
  const items: (Workflow | Routine)[] = routines
    ? (data.data?.routines ?? [])
    : (data.data?.workflows ?? []);
  const runs = (data.data?.runs ?? []).filter((run) =>
    routines
      ? !!(run.routine_id || run.route_id)
      : !run.routine_id && !run.route_id,
  );
  const title = routines ? "Routines" : "Workflows";
  const workspaceRows = workspaces.data ?? [];
  const unavailable = !workspaceRows.length;
  function edit(value: Editor) {
    change.reset();
    setOperationError("");
    setEditor(value);
  }
  function create() {
    const value = {
      id: crypto.randomUUID(),
      revision: 0,
      name: "",
      workspace_id: workspaceRows[0].workspace.workspace_id,
      instructions: "",
    };
    edit(
      routines
        ? {
            kind: "routine",
            value: {
              ...value,
              enabled: true,
              first_run_ms: Date.now() + 3600_000,
              next_run_ms: 0,
              interval_minutes: 1440,
            },
          }
        : { kind: "workflow", value },
    );
  }
  async function run(item: Workflow) {
    setOperationError("");
    setPendingItem(`run:${item.id}`);
    const key = `cuttlefish.run.${item.id}`;
    try {
      const runId = sessionStorage.getItem(key) ?? crypto.randomUUID();
      sessionStorage.setItem(key, runId);
      await change.mutateAsync(
        routines
          ? { type: "run_routine", routine_id: item.id, run_id: runId }
          : { type: "run", workflow_id: item.id, run_id: runId },
      );
      sessionStorage.removeItem(key);
      setAnnouncement(`Run requested for ${item.name}.`);
    } catch (error) {
      setOperationError(errorMessage(error));
    }
  }
  function toggle(item: Routine) {
    setOperationError("");
    setPendingItem(`toggle:${item.id}`);
    change.mutate(
      {
        type: "save_routine",
        routine: { ...routineDefinition(item), enabled: !item.enabled },
      },
      {
        onError: (error) => setOperationError(errorMessage(error)),
        onSuccess: () =>
          setAnnouncement(
            item.enabled ? "Routine paused." : "Routine resumed.",
          ),
      },
    );
  }
  function save(body: unknown) {
    change.mutate(body, {
      onSuccess: () => {
        setEditor(null);
        setAnnouncement("Saved.");
      },
    });
  }
  return (
    <main
      className="automation-page automation-workspace"
      aria-labelledby={`${section}-heading`}
    >
      <header className="section-header">
        <h1 id={`${section}-heading`} tabIndex={-1}>
          {title}
        </h1>
        <Button
          onClick={create}
          disabled={
            unsupported ||
            unavailable ||
            change.isPending ||
            data.isPending ||
            !!data.error
          }
          className="automation-touch-target"
        >
          <Plus data-icon="inline-start" />
          New {routines ? "routine" : "workflow"}
        </Button>
      </header>
      <p className="sr-only" role="status">
        {announcement}
      </p>
      {data.isPending && (
        <p role="status" className="automation-empty">
          Loading {title.toLowerCase()}…
        </p>
      )}
      {(data.error || workspaces.error) && (
        <p role="alert" className="error">
          {errorMessage(data.error ?? workspaces.error)}
        </p>
      )}
      {operationError && !editor && (
        <p role="alert" className="error automation-feedback">
          {operationError}
        </p>
      )}
      {unsupported ? (
        <div className="automation-empty" role="status">
          <h2>Update required</h2>
          <p>Update this installation to create and manage routines.</p>
        </div>
      ) : (
        <>
          {data.data && items.length === 0 && (
            <div className="automation-empty">
              {routines ? (
                <Repeat2 aria-hidden="true" />
              ) : (
                <WorkflowIcon aria-hidden="true" />
              )}
              <h2>No {title.toLowerCase()} yet</h2>
              <p>
                {unavailable
                  ? "Add an agent workspace in Settings to get started."
                  : routines
                    ? "Give an agent a prompt and choose when it runs."
                    : "Save a prompt you want to run again."}
              </p>
            </div>
          )}
          <ul
            className="automation-list automation-definitions"
            aria-label={title}
          >
            {items.map((item) => {
              const scheduled = "enabled" in item ? item : null;
              const workspace = workspaceRows.find(
                ({ workspace }) => workspace.workspace_id === item.workspace_id,
              )?.workspace;
              return (
                <li key={item.id}>
                  <div className="automation-item-content">
                    <div className="automation-item-title">
                      <h2>{item.name}</h2>
                      {scheduled && (
                        <span className="automation-status">
                          {scheduled.enabled ? "Active" : "Paused"}
                        </span>
                      )}
                    </div>
                    <p className="automation-prompt">{item.instructions}</p>
                    <div className="automation-metadata">
                      <span>
                        {workspace
                          ? `${workspace.name} · ${providerLabel(workspace.provider)}`
                          : "Workspace unavailable"}
                      </span>
                      {scheduled && (
                        <>
                          <span>
                            <Repeat2 aria-hidden="true" />
                            {intervalLabel(scheduled.interval_minutes)}
                          </span>
                          {scheduled.enabled && (
                            <span>
                              <Clock3 aria-hidden="true" />
                              Next {formatTime(scheduled.next_run_ms)}
                            </span>
                          )}
                        </>
                      )}
                    </div>
                  </div>
                  <div className="row-actions automation-item-actions">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="automation-touch-target"
                      disabled={change.isPending}
                      aria-label={`Edit ${item.name}`}
                      title={`Edit ${item.name}`}
                      onClick={() =>
                        edit(
                          scheduled
                            ? { kind: "routine", value: scheduled }
                            : { kind: "workflow", value: item },
                        )
                      }
                    >
                      <Pencil />
                    </Button>
                    {scheduled && (
                      <Button
                        variant="ghost"
                        className="automation-touch-target"
                        disabled={change.isPending}
                        aria-label={`${change.isPending && pendingItem === `toggle:${item.id}` ? (scheduled.enabled ? "Pausing…" : "Resuming…") : scheduled.enabled ? "Pause" : "Resume"} ${item.name}`}
                        onClick={() => toggle(scheduled)}
                      >
                        {scheduled.enabled ? (
                          <Pause data-icon="inline-start" />
                        ) : (
                          <Play data-icon="inline-start" />
                        )}
                        {change.isPending && pendingItem === `toggle:${item.id}`
                          ? scheduled.enabled
                            ? "Pausing…"
                            : "Resuming…"
                          : scheduled.enabled
                            ? "Pause"
                            : "Resume"}
                      </Button>
                    )}
                    <Button
                      variant="outline"
                      className="automation-touch-target"
                      disabled={change.isPending || !workspace}
                      aria-label={`Run ${item.name}`}
                      onClick={() => void run(item)}
                    >
                      <Play data-icon="inline-start" />
                      {change.isPending && pendingItem === `run:${item.id}`
                        ? "Starting…"
                        : "Run now"}
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
          {!!runs.length && (
            <section
              className="run-history"
              aria-labelledby={`${section}-runs`}
            >
              <h2 id={`${section}-runs`}>Recent runs</h2>
              <ul className="automation-list automation-runs">
                {runs.map((run) => {
                  const item = items.find(
                    (item) =>
                      item.id ===
                      (routines
                        ? run.routine_id || run.route_id
                        : run.workflow_id),
                  );
                  const status = runStatus(run.state);
                  return (
                    <li key={run.id}>
                      <div className="automation-run-content">
                        <h3>
                          {item?.name ?? (routines ? "Routine" : "Workflow")}
                        </h3>
                        <time dateTime={new Date(run.created_ms).toISOString()}>
                          {formatTime(run.created_ms)}
                        </time>
                        {run.error && (
                          <p className="automation-run-error">{run.error}</p>
                        )}
                      </div>
                      <span
                        className="automation-run-status"
                        data-state={run.state}
                      >
                        {status}
                      </span>
                      <Button
                        variant="ghost"
                        className="automation-touch-target"
                        disabled={run.state === "pending"}
                        onClick={() => onOpenChat(run.thread_id)}
                        aria-label={`Open ${item?.name ?? "run"} conversation`}
                      >
                        Open chat
                        <ArrowUpRight data-icon="inline-end" />
                      </Button>
                    </li>
                  );
                })}
              </ul>
            </section>
          )}
        </>
      )}
      {editor && (
        <AutomationEditor
          key={`${editor.kind}-${editor.value.id}`}
          editor={editor}
          workspaces={workspaceRows}
          pending={change.isPending}
          error={change.error ? errorMessage(change.error) : ""}
          onClose={() => setEditor(null)}
          onSave={save}
        />
      )}
    </main>
  );
}
