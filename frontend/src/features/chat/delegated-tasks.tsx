import {
  Check,
  Clock3,
  LoaderCircle,
  ArrowUpRight,
  TriangleAlert,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import type { z } from "@/lib/schema";
import { childTaskSchema } from "@/lib/contracts";
import { providerLabel } from "@/lib/activity";
export function DelegatedTasks({
  tasks,
  error,
  onOpen,
}: {
  tasks: z.infer<typeof childTaskSchema>[];
  error: boolean;
  onOpen: (thread: string) => void;
}) {
  if (error)
    return (
      <p role="status" className="text-sm text-muted-foreground">
        Delegated task updates are temporarily unavailable.
      </p>
    );
  if (!tasks.length) return null;
  return (
    <section
      className="mb-6 rounded-lg border p-4"
      aria-label="Delegated tasks"
    >
      <h2 className="mb-2 text-sm font-medium">Delegated tasks</h2>
      <ul className="divide-y">
        {tasks.map((task) => (
          <li key={task.task_id} className="flex items-start gap-3 py-3">
            {task.outcome?.type === "rejected" ? (
              <TriangleAlert size={16} />
            ) : task.state === "completed" ? (
              <Check size={16} />
            ) : task.state === "claimed" ? (
              <LoaderCircle size={16} className="spinning" />
            ) : (
              <Clock3 size={16} />
            )}
            <div className="min-w-0 flex-1">
              <p className="text-sm break-words">{task.title}</p>
              <p className="mt-1 text-xs text-muted-foreground">
                {task.workspace || "Awaiting assignment"}
                {task.provider && ` · ${providerLabel(task.provider)}`}
                {task.platform && ` · ${platformLabel(task.platform)}`} ·{" "}
                {task.outcome?.type === "rejected"
                  ? "Needs attention"
                  : task.state === "claimed"
                    ? "Working"
                    : task.state === "completed"
                      ? "Finished"
                      : "Queued"}
              </p>
            </div>
            <Button
              size="icon"
              variant="ghost"
              disabled={!task.node_id}
              onClick={() => onOpen(task.thread_id)}
              aria-label={`Open task: ${task.title}`}
            >
              <ArrowUpRight size={16} />
            </Button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function platformLabel(platform: string) {
  const names: Record<string, string> = {
    mac_os: "macOS",
    windows: "Windows",
    linux: "Linux",
  };
  return names[platform] ?? platform;
}
