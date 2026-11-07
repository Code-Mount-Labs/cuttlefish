import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Play, Pause, Pencil } from "lucide-react";
import { z } from "@/lib/schema";
import { local, errorMessage } from "@/lib/api";
import { workspaceRowSchema } from "@/lib/contracts";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { FieldGroup } from "@/components/ui/field";
import { FormField } from "@/components/form-field";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";

const workflowSchema = z.object({
  id: z.string().regex(/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i),
  revision: z.number().int(),
  name: z.string(),
  workspace_id: z
    .string()
    .regex(/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i),
  instructions: z.string(),
});
const routeSchema = z.object({
  id: z.string().regex(/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i),
  revision: z.number().int(),
  name: z.string(),
  workflow_id: z
    .string()
    .regex(/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i),
  enabled: z.boolean(),
  first_run_ms: z.number(),
  interval_minutes: z.number(),
  next_run_ms: z.number(),
});
const dataSchema = z.object({
  workflows: z.array(workflowSchema),
  routes: z.array(routeSchema),
  runs: z.array(
    z.object({
      id: z.string(),
      workflow_id: z.string(),
      route_id: z.string().nullable(),
      thread_id: z.string(),
      created_ms: z.number(),
      state: z.string(),
      error: z.string().nullable(),
    }),
  ),
});
type Workflow = z.infer<typeof workflowSchema>;
type Route = z.infer<typeof routeSchema>;
type Editor =
  { kind: "workflow"; value: Workflow } | { kind: "route"; value: Route };

function intervalLabel(minutes: number) {
  const labels: Record<number, string> = {
    60: "Every hour",
    1440: "Every 24 hours",
    10080: "Every 7 days",
  };
  return labels[minutes] ?? `Every ${minutes} minutes`;
}
function localTime(ms: number) {
  const date = new Date(ms);
  return new Date(ms - date.getTimezoneOffset() * 60_000)
    .toISOString()
    .slice(0, 16);
}
export function AutomationPage({
  section,
  onOpenChat,
}: {
  section: "routes" | "workflows";
  onOpenChat: (id: string) => void;
}) {
  const cache = useQueryClient();
  const [editor, setEditor] = useState<Editor | null>(null);
  const [notice, setNotice] = useState("");
  const data = useQuery({
    queryKey: ["automations"],
    queryFn: ({ signal }) => local("automations", dataSchema, { signal }),
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
  const workflows = data.data?.workflows ?? [];
  const routes = data.data?.routes ?? [];
  const runs = (data.data?.runs ?? []).filter(
    (run) => section === "workflows" || run.route_id,
  );
  const title = section === "workflows" ? "Workflows" : "Routes";
  function create() {
    setNotice("");
    change.reset();
    const id = crypto.randomUUID();
    setEditor(
      section === "workflows"
        ? {
            kind: "workflow",
            value: {
              id,
              revision: 0,
              name: "",
              workspace_id: workspaces.data?.[0]?.workspace.workspace_id ?? "",
              instructions: "",
            },
          }
        : {
            kind: "route",
            value: {
              id,
              revision: 0,
              name: "",
              workflow_id: workflows[0].id,
              enabled: true,
              first_run_ms: Date.now() + 3600_000,
              next_run_ms: 0,
              interval_minutes: 1440,
            },
          },
    );
  }
  async function run(workflow: Workflow) {
    setNotice("");
    const key = `cuttlefish.run.${workflow.id}`;
    try {
      const runId = sessionStorage.getItem(key) ?? crypto.randomUUID();
      sessionStorage.setItem(key, runId);
      await change.mutateAsync({
        type: "run",
        workflow_id: workflow.id,
        run_id: runId,
      });
      sessionStorage.removeItem(key);
      setNotice(
        `${workflow.name} queued. Open the run below to follow its progress.`,
      );
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }
  function submit(form: HTMLFormElement, editor: Editor) {
    const fields = new FormData(form);
    const name = String(fields.get("name")).trim();
    const body =
      editor.kind === "workflow"
        ? {
            type: "save_workflow",
            workflow: {
              ...editor.value,
              name,
              workspace_id: String(fields.get("workspace")),
              instructions: String(fields.get("instructions")),
            },
          }
        : {
            type: "save_route",
            route: {
              id: editor.value.id,
              revision: editor.value.revision,
              enabled: editor.value.enabled,
              name,
              workflow_id: String(fields.get("workflow")),
              first_run_ms: new Date(String(fields.get("first"))).getTime(),
              interval_minutes: Number(fields.get("interval")),
            },
          };
    change.mutate(body, {
      onSuccess: () => {
        setEditor(null);
        setNotice("Saved");
      },
    });
  }
  function toggle(route: Route) {
    const { next_run_ms: next, ...saved } = route;
    void next;
    change.mutate({
      type: "save_route",
      route: { ...saved, enabled: !route.enabled },
    });
  }
  return (
    <main className="automation-page">
      <header className="section-header">
        <h1>{title}</h1>
        <Button
          onClick={create}
          disabled={section === "routes" && !workflows.length}
        >
          <Plus data-icon="inline-start" />
          New {section === "routes" ? "route" : "workflow"}
        </Button>
      </header>
      {data.isPending && <p role="status">Loading…</p>}
      {(data.error || workspaces.error) && (
        <p role="alert">{errorMessage(data.error ?? workspaces.error)}</p>
      )}
      {notice && <p role="status">{notice}</p>}
      {change.error && !editor && (
        <p role="alert">{errorMessage(change.error)}</p>
      )}
      {section === "workflows" && (
        <ul className="automation-list">
          {workflows.map((workflow) => (
            <li key={workflow.id}>
              <div>
                <h2>{workflow.name}</h2>
                <p className="muted">
                  {workspaces.data?.find(
                    (w) => w.workspace.workspace_id === workflow.workspace_id,
                  )?.workspace.name ?? "Workspace unavailable"}
                </p>
              </div>
              <div className="row-actions">
                <Button
                  variant="ghost"
                  aria-label={`Edit ${workflow.name}`}
                  onClick={() => {
                    change.reset();
                    setEditor({ kind: "workflow", value: workflow });
                  }}
                >
                  <Pencil />
                </Button>
                <Button
                  variant="outline"
                  disabled={change.isPending}
                  onClick={() => void run(workflow)}
                >
                  <Play data-icon="inline-start" />
                  Run
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {section === "routes" && (
        <ul className="automation-list">
          {routes.map((route) => (
            <li key={route.id}>
              <div>
                <h2>{route.name}</h2>
                <p className="muted">
                  {route.enabled
                    ? `Next run ${new Date(route.next_run_ms).toLocaleString()}`
                    : "Paused"}
                </p>
                <p className="muted">
                  {intervalLabel(route.interval_minutes)} ·{" "}
                  {workflows.find((w) => w.id === route.workflow_id)?.name}
                </p>
              </div>
              <div className="row-actions">
                <Button
                  variant="ghost"
                  aria-label={`Edit ${route.name}`}
                  onClick={() => {
                    change.reset();
                    setEditor({ kind: "route", value: route });
                  }}
                >
                  <Pencil />
                </Button>
                <Button
                  variant="outline"
                  disabled={change.isPending}
                  onClick={() => toggle(route)}
                >
                  {route.enabled ? (
                    <Pause data-icon="inline-start" />
                  ) : (
                    <Play data-icon="inline-start" />
                  )}
                  {route.enabled ? "Pause" : "Resume"}
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {data.data &&
        (section === "workflows" ? !workflows.length : !routes.length) && (
          <p className="empty-section">
            {section === "workflows"
              ? "Save instructions as a workflow, then run them whenever you need."
              : workflows.length
                ? "Schedule a workflow to run automatically."
                : "Create a workflow first, then schedule it here."}
          </p>
        )}
      {!!runs.length && (
        <section className="run-history">
          <h2>Recent runs</h2>
          <ul className="automation-list">
            {runs.map((run) => (
              <li key={run.id}>
                <div>
                  <strong>
                    {workflows.find((w) => w.id === run.workflow_id)?.name ??
                      "Workflow"}
                  </strong>
                  <p className="muted">
                    {new Date(run.created_ms).toLocaleString()} ·{" "}
                    {run.state === "pending"
                      ? "Waiting to dispatch"
                      : run.state}
                  </p>
                  {run.error && <p className="muted">{run.error}</p>}
                </div>
                <Button
                  variant="ghost"
                  disabled={run.state === "pending"}
                  onClick={() => onOpenChat(run.thread_id)}
                >
                  Open chat
                </Button>
              </li>
            ))}
          </ul>
        </section>
      )}
      <Dialog
        open={!!editor}
        onOpenChange={(open) => {
          if (!open && !change.isPending) setEditor(null);
        }}
      >
        <DialogContent aria-describedby="automation-description">
          <DialogTitle>
            {editor?.value.revision ? "Edit" : "New"} {editor?.kind}
          </DialogTitle>
          <DialogDescription id="automation-description">
            {editor?.kind === "workflow"
              ? "Choose the agent workspace and instructions for each fresh run."
              : "Times are shown in your local time zone. Repeats use elapsed time, including across daylight-saving changes. Missed runs are combined and overlapping runs are skipped."}
          </DialogDescription>
          {editor && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                submit(event.currentTarget, editor);
              }}
            >
              <FieldGroup>
                <FormField label="Name">
                  {(id) => (
                    <Input
                      id={id}
                      name="name"
                      defaultValue={editor.value.name}
                      maxLength={128}
                      required
                    />
                  )}
                </FormField>
                {editor.kind === "workflow" ? (
                  <>
                    <FormField label="Agent workspace">
                      {(id) => (
                        <NativeSelect
                          id={id}
                          name="workspace"
                          defaultValue={editor.value.workspace_id}
                          required
                        >
                          {workspaces.data?.map((w) => (
                            <NativeSelectOption
                              key={w.workspace.workspace_id}
                              value={w.workspace.workspace_id}
                            >
                              {w.workspace.name} · {w.workspace.provider}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                      )}
                    </FormField>
                    <FormField label="Instructions">
                      {(id) => (
                        <Textarea
                          id={id}
                          name="instructions"
                          defaultValue={editor.value.instructions}
                          rows={6}
                          maxLength={4000}
                          required
                        />
                      )}
                    </FormField>
                  </>
                ) : (
                  <>
                    <FormField label="Workflow">
                      {(id) => (
                        <NativeSelect
                          id={id}
                          name="workflow"
                          defaultValue={editor.value.workflow_id}
                          required
                        >
                          {workflows.map((w) => (
                            <NativeSelectOption key={w.id} value={w.id}>
                              {w.name}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                      )}
                    </FormField>
                    <FormField label="First run">
                      {(id) => (
                        <Input
                          id={id}
                          name="first"
                          type="datetime-local"
                          defaultValue={localTime(editor.value.first_run_ms)}
                          required
                        />
                      )}
                    </FormField>
                    <FormField label="Repeat">
                      {(id) => (
                        <NativeSelect
                          id={id}
                          name="interval"
                          defaultValue={editor.value.interval_minutes}
                        >
                          {[
                            ...new Set([
                              60,
                              1440,
                              10080,
                              editor.value.interval_minutes,
                            ]),
                          ].map((minutes) => (
                            <NativeSelectOption key={minutes} value={minutes}>
                              {intervalLabel(minutes)}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                      )}
                    </FormField>
                  </>
                )}
                {change.error && (
                  <p role="alert">{errorMessage(change.error)}</p>
                )}
                <Button type="submit" disabled={change.isPending}>
                  {change.isPending ? "Saving…" : "Save"}
                </Button>
              </FieldGroup>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </main>
  );
}
