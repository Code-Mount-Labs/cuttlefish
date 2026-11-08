import { z } from "@/lib/schema";

const identity = z
  .string()
  .regex(/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i);
const taskFields = {
  id: identity,
  revision: z.number().int(),
  name: z.string(),
  workspace_id: identity,
  instructions: z.string(),
};
export const workflowSchema = z.object(taskFields);
export const routineSchema = z.object({
  ...taskFields,
  enabled: z.boolean(),
  first_run_ms: z.number(),
  interval_minutes: z.number(),
  next_run_ms: z.number(),
});
export const automationSchema = z.object({
  workflows: z.array(workflowSchema),
  routines: z.array(routineSchema).optional(),
  runs: z.array(
    z.object({
      id: z.string(),
      workflow_id: z.string().nullable(),
      routine_id: z.string().nullable().optional(),
      route_id: z.string().nullable().optional(),
      thread_id: z.string(),
      created_ms: z.number(),
      state: z.string(),
      error: z.string().nullable(),
    }),
  ),
});
export type Workflow = z.infer<typeof workflowSchema>;
export type Routine = z.infer<typeof routineSchema>;
export type AutomationData = z.infer<typeof automationSchema>;
export type Section = "routines" | "workflows";
export type Editor =
  { kind: "workflow"; value: Workflow } | { kind: "routine"; value: Routine };
export function intervalLabel(minutes: number) {
  const labels: Record<number, string> = {
    15: "Every 15 minutes",
    60: "Every hour",
    1440: "Every 24 hours",
    10080: "Every 7 days",
  };
  return labels[minutes] ?? `Every ${minutes} minutes`;
}
export function localTime(ms: number) {
  const date = new Date(ms);
  return new Date(ms - date.getTimezoneOffset() * 60_000)
    .toISOString()
    .slice(0, 16);
}
export function runStatus(state: string) {
  const labels: Record<string, string> = {
    pending: "Queued",
    queued: "Queued",
    claimed: "Running",
    completed: "Completed",
    failed: "Failed",
    cancelled: "Stopped",
  };
  return labels[state] ?? "Awaiting an update";
}
export function routineDefinition(routine: Routine) {
  const { next_run_ms: next, ...definition } = routine;
  void next;
  return definition;
}
export function formatTime(ms: number) {
  return new Date(ms).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

export function firstRunTime(original: number, input: string) {
  return input === localTime(original) ? original : new Date(input).getTime();
}
export function schedulePreview(
  routine: Routine,
  input: string,
  interval: number,
  now: number,
) {
  const first = firstRunTime(routine.first_run_ms, input);
  if (!Number.isFinite(first)) return "Choose a valid first-run time.";
  if (!routine.enabled)
    return "Paused. This routine will not run until resumed.";
  const next =
    first === routine.first_run_ms && interval === routine.interval_minutes
      ? routine.next_run_ms
      : first;
  return next <= now
    ? "Due now. Runs when the router is available; missed runs are combined."
    : `Next run ${formatTime(next)}.`;
}
