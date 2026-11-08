import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../../lib/schema";
import { ApiError, local, errorMessage } from "../../lib/api";
import {
  activitySchema,
  childTaskSchema,
  conversationSchema,
  turnSchema,
  workspaceRowSchema,
  type Request,
} from "../../lib/contracts";
import { readPending, savePending } from "../../lib/pending";
import { usePendingRequest } from "../../hooks/use-pending-request";
import {
  MAX_FILES,
  MAX_FILE_SIZE,
  uploadFiles,
  type DraftFile,
} from "../../lib/files";
export function useChat(
  enabled: boolean,
  activityEnabled: boolean,
  collaborationEnabled = false,
) {
  const cache = useQueryClient();
  const pending = usePendingRequest();
  const [thread, setThread] = useState<string | null>(
    () =>
      readPending()?.thread_id ??
      location.hash.match(/^#chat\/([a-f0-9-]{36})$/)?.[1] ??
      null,
  );
  const [text, setText] = useState("");
  const [files, setFiles] = useState<DraftFile[]>([]),
    [selected, setSelected] = useState(""),
    [progress, setProgress] = useState(""),
    [error, setError] = useState("");
  const workspaces = useQuery({
    queryKey: ["workspaces"],
    queryFn: ({ signal }) =>
      local("chat/workspaces", z.array(workspaceRowSchema), { signal }),
    enabled,
    refetchInterval: 5000,
  });
  const conversations = useQuery({
    queryKey: ["conversations"],
    queryFn: ({ signal }) =>
      local("chat", z.array(conversationSchema), { signal }),
    enabled,
    refetchInterval: 5000,
  });
  const turns = useQuery({
    queryKey: ["turns", thread],
    queryFn: ({ signal }) =>
      local("chat/" + thread, z.array(turnSchema), { signal }),
    enabled: enabled && !!thread,
    refetchInterval: 2000,
  });
  const activity = useQuery({
    queryKey: ["activity", thread],
    queryFn: ({ signal }) =>
      local("chat/" + thread + "/activity", activitySchema, { signal }),
    enabled: enabled && !!thread && activityEnabled,
    refetchInterval: 2000,
  });
  const children = useQuery({
    queryKey: ["children", thread],
    queryFn: ({ signal }) =>
      local("chat/" + thread + "/tasks", z.array(childTaskSchema), { signal }),
    enabled: enabled && !!thread && collaborationEnabled,
    refetchInterval: 2000,
  });
  const current = conversations.data?.find((c) => c.thread_id === thread);
  const workspace =
    current?.workspace_id ??
    (pending?.action.type === "start" ? pending.action.workspace_id : null) ??
    selected;
  const effectiveWorkspace =
    workspace || workspaces.data?.[0]?.workspace.workspace_id || "";
  const acceptedPending =
    pending &&
    turns.data?.some((t) => t.request.command_id === pending.command_id);
  // Reconcile an acknowledgement lost during reload before allowing another send.
  const unresolved = acceptedPending ? null : pending;
  useEffect(() => {
    if (acceptedPending) savePending(null);
  }, [acceptedPending]);
  function updatePending(value: Request | null) {
    savePending(value);
  }
  function navigate(id: string | null) {
    setThread(id);
    setText("");
    setFiles([]);
    setError("");
    history.replaceState(null, "", id ? "#chat/" + id : location.pathname);
  }
  const send = useMutation({
    mutationFn: async (cancel: boolean) => {
      setError("");
      setProgress("Preparing your message…");
      let request = unresolved;
      if (!request) {
        if (cancel) {
          if (!thread) throw new Error("Select a chat first.");
          request = {
            command_id: crypto.randomUUID(),
            thread_id: thread,
            action: { type: "cancel" },
          };
        } else {
          const row = workspaces.data?.find(
            (r) => r.workspace.workspace_id === effectiveWorkspace,
          );
          if (!row) throw new Error("Select a workspace first.");
          const attachments = await uploadFiles(
            files,
            effectiveWorkspace,
            setProgress,
          );
          const message = {
            id: crypto.randomUUID(),
            text: text.trim() || "Please examine the attached files.",
            attachments,
          };
          request = {
            command_id: crypto.randomUUID(),
            thread_id: thread ?? crypto.randomUUID(),
            action: thread
              ? { type: "send", message }
              : {
                  type: "start",
                  task_id: crypto.randomUUID(),
                  session_id: crypto.randomUUID(),
                  workspace_id: effectiveWorkspace,
                  capabilities: [row.workspace.provider],
                  message,
                },
          };
        }
        updatePending(request);
      }
      setProgress("Sending task to your agent…");
      const reply = await local("chat", turnSchema, { body: request });
      updatePending(null);
      setText("");
      setFiles([]);
      setThread(reply.request.thread_id);
      history.replaceState(null, "", "#chat/" + reply.request.thread_id);
    },
    onError: (e) => {
      setError(errorMessage(e));
      if (e instanceof ApiError && e.status === 400) {
        const rejected = readPending();
        if (rejected && "message" in rejected.action)
          setText(rejected.action.message.text);
        updatePending(null);
      }
    },
    onSettled: async () => {
      setProgress("");
      await Promise.all([
        cache.invalidateQueries({ queryKey: ["turns"] }),
        cache.invalidateQueries({ queryKey: ["conversations"] }),
        cache.invalidateQueries({ queryKey: ["activity"] }),
      ]);
    },
  });
  function addFiles(incoming: File[]) {
    if (send.isPending || unresolved) return;
    const next = [...files];
    for (const file of incoming) {
      if (next.length >= MAX_FILES) {
        setError("Attach up to 8 files per message.");
        break;
      }
      if (file.size > MAX_FILE_SIZE) {
        setError(`${file.name} exceeds the 8 MiB file limit.`);
        continue;
      }
      next.push({ id: crypto.randomUUID(), file });
    }
    setFiles(next);
  }
  const latest = turns.data?.at(-1),
    busy = !!latest && latest.state !== "completed";
  const ended =
    turns.data?.some(
      (t) =>
        t.outcome?.type === "completed" &&
        t.outcome.event.kind.type === "cancelled",
    ) ?? false;
  return {
    thread,
    navigate,
    text:
      unresolved && "message" in unresolved.action
        ? unresolved.action.message.text
        : text,
    setText,
    files,
    addFiles,
    removeFile: (id: string) => setFiles(files.filter((f) => f.id !== id)),
    workspace: effectiveWorkspace,
    setWorkspace: setSelected,
    workspaces,
    conversations,
    turns,
    activity,
    children,
    pending: unresolved,
    sending: send.isPending,
    send: () => send.mutate(false),
    end: () => send.mutate(true),
    busy,
    ended,
    progress,
    error,
    queryError:
      workspaces.error ?? conversations.error ?? turns.error ?? activity.error,
  };
}
