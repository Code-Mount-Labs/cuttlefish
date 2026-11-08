import { updateSchema } from "./contracts";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { z } from "../../lib/schema";
import { local, errorMessage } from "../../lib/api";
import { Button } from "../../components/ui/button";
import { RefreshCw } from "lucide-react";

type Update = z.infer<typeof updateSchema>;
const active = (data: Update | undefined) =>
  !!data?.job &&
  ["queued", "downloading", "restarting"].includes(data.job.phase);
const messages = {
  queued: "Preparing…",
  downloading: "Downloading and verifying the update…",
  restarting: "Restarting Cuttlefish. Reconnecting…",
  complete: "Ready. Your chats and settings are unchanged.",
  restart_required:
    "Update installed. Restart your foreground Cuttlefish process to finish.",
  failed: "The update could not finish. Retry to continue.",
};

export function UpdatesPanel({ enabled }: { enabled: boolean }) {
  const cache = useQueryClient();
  const mutation = useMutation({
    mutationFn: (body: { action: string; version?: string }) =>
      local("updates", updateSchema, { body }),
    onSuccess: (data) => cache.setQueryData(["updates"], data),
    onSettled: () => cache.invalidateQueries({ queryKey: ["updates"] }),
  });
  const query = useQuery({
    queryKey: ["updates"],
    queryFn: ({ signal }) => local("updates", updateSchema, { signal }),
    enabled,
    refetchInterval: (query) => {
      // A restart can close the POST connection before its response arrives.
      // Reconcile with the durable job until a fresh status is available.
      const unconfirmed =
        mutation.isError &&
        mutation.variables.action !== "check" &&
        query.state.dataUpdatedAt <= mutation.submittedAt;
      return active(query.state.data) || unconfirmed ? 1500 : false;
    },
    refetchOnWindowFocus: false,
  });
  const data = query.data;
  const busy = mutation.isPending || active(data);
  const error = mutation.error ?? (!active(data) ? query.error : null);
  return (
    <section className="settings-section" aria-labelledby="updates-title">
      <h2 id="updates-title">Updates</h2>
      {!enabled ? (
        <p>
          Install the latest Cuttlefish release once to enable dashboard
          updates.
        </p>
      ) : !data ? (
        <p role="status">Checking installed version…</p>
      ) : (
        <>
          <p>
            {data.current_version}
            {data.available_version
              ? ` · ${data.available_version} available`
              : ""}
          </p>
          {!data.supported ? (
            <p>
              Use the Cuttlefish installer to enable managed updates for this
              installation.
            </p>
          ) : (
            <>
              <div className="form-actions">
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => mutation.mutate({ action: "check" })}
                >
                  <RefreshCw data-icon="inline-start" />
                  {mutation.isPending && mutation.variables.action === "check"
                    ? "Checking…"
                    : "Check for updates"}
                </Button>
                {data.available_version && (
                  <Button
                    disabled={busy}
                    onClick={() =>
                      mutation.mutate({
                        action: "install",
                        version: data.available_version!,
                      })
                    }
                  >
                    {data.automatic_restart
                      ? "Update and restart"
                      : "Install update"}
                  </Button>
                )}
                {data.automatic_restart && !data.available_version && (
                  <Button
                    variant="outline"
                    disabled={busy}
                    onClick={() => mutation.mutate({ action: "restart" })}
                  >
                    Restart Cuttlefish
                  </Button>
                )}
                {(data.job?.phase === "failed" ||
                  data.job?.phase === "restart_required") && (
                  <Button
                    variant="outline"
                    disabled={busy}
                    onClick={() => mutation.mutate({ action: "retry" })}
                  >
                    Retry
                  </Button>
                )}
              </div>
              {data.job ? (
                <p role="status" aria-live="polite">
                  {messages[data.job.phase]}
                </p>
              ) : data.checked &&
                !data.available_version &&
                !data.check_error ? (
                <p role="status">You’re up to date.</p>
              ) : null}
              {data.job?.phase === "complete" && !data.job.restart_only && (
                <Button variant="outline" onClick={() => location.reload()}>
                  Reload to use the new version
                </Button>
              )}
              {!data.automatic_restart && (
                <p>
                  Background startup is not enabled. Updates will require a
                  manual restart.
                </p>
              )}
            </>
          )}
          {(data.check_error || data.job?.error) && (
            <p role="alert">{data.check_error || data.job?.error}</p>
          )}
        </>
      )}
      {error && <p role="alert">{errorMessage(error)}</p>}
      {query.error && !data && (
        <Button variant="outline" onClick={() => void query.refetch()}>
          Retry
        </Button>
      )}
    </section>
  );
}
