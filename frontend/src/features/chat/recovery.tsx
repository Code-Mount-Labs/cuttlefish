import { useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { LoaderCircle, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { local, errorMessage } from "@/lib/api";
import { recoverySchema } from "@/lib/contracts";
import { providerLabel } from "@/lib/activity";

export function TaskRecovery({
  thread,
  command,
}: {
  thread: string;
  command: string;
}) {
  const client = useQueryClient();
  const pending = useRef<string | null>(null);
  const key = ["recovery", thread];
  const path = `chat/${thread}/recovery`;
  const query = useQuery({
    queryKey: key,
    queryFn: ({ signal }) => local(path, recoverySchema.array(), { signal }),
    refetchInterval: 2000,
  });
  const view = query.data?.find((v) => v.command_id === command);
  const action = useMutation({
    mutationFn: (body: unknown) => local(path, recoverySchema, { body }),
    onSuccess: () => {
      pending.current = null;
      void client.invalidateQueries({ queryKey: key });
    },
  });
  const busy =
    action.isPending ||
    view?.state === "inspecting" ||
    view?.state === "approved";
  const inspect = () => {
    pending.current ??= crypto.randomUUID();
    action.mutate({
      type: "inspect",
      id: pending.current,
      command_id: command,
    });
  };
  return (
    <section
      className="my-3 rounded-lg border p-4 text-sm"
      aria-label="Recover interrupted task"
    >
      <h3 className="font-medium">Continue from saved work</h3>
      <p className="mt-2 text-muted-foreground">
        Keep this chat, workspace and provider session. Inspect the checkpoint
        before continuing unfinished work.
      </p>
      {query.isPending && (
        <p role="status" className="mt-3">
          Checking recovery status…
        </p>
      )}
      {(query.error || action.error) && (
        <p role="alert" className="mt-3 text-destructive">
          {errorMessage(action.error || query.error)}
        </p>
      )}
      {view?.checkpoint && (
        <div className="mt-3 space-y-2">
          <p>
            {providerLabel(view.checkpoint.provider)} · Saved session ·{" "}
            {view.checkpoint.files} project files found
          </p>
          {view.checkpoint.progress && (
            <p className="text-muted-foreground">
              Last update: {view.checkpoint.progress}
            </p>
          )}
        </div>
      )}
      {view?.detail && (
        <p role="status" className="mt-3 text-muted-foreground">
          {view.detail}
        </p>
      )}
      {busy ? (
        <p role="status" className="mt-3 flex items-center gap-2">
          <LoaderCircle size={14} className="animate-spin" />
          {view?.state === "approved"
            ? "Continuation approved. Waiting for the original agent…"
            : "Waiting for the original agent to inspect saved state…"}
        </p>
      ) : view?.state === "ready" && view.checkpoint ? (
        <div className="mt-3 space-y-3">
          <p className="text-muted-foreground">
            The agent will inspect existing files and task history, verify
            uncertain results, then continue what remains. It will not rerun the
            original request from the beginning.
          </p>
          <Button
            size="sm"
            onClick={() =>
              action.mutate({
                type: "continue",
                id: view.id,
                digest: view.checkpoint!.digest,
              })
            }
          >
            <RotateCcw />
            Continue from checkpoint
          </Button>
        </div>
      ) : view?.state === "resumed" ? (
        <div className="mt-3 space-y-3">
          <p role="status">
            Continuation saved. If it stopped again, inspect a new checkpoint.
          </p>
          <Button size="sm" variant="outline" onClick={inspect}>
            Inspect again
          </Button>
        </div>
      ) : (
        <Button
          className="mt-3"
          size="sm"
          variant="outline"
          disabled={query.isPending}
          onClick={inspect}
        >
          {view?.state === "blocked" ? "Inspect again" : "Inspect saved work"}
        </Button>
      )}
    </section>
  );
}
