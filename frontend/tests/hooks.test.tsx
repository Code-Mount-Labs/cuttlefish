import { describe, it, expect, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useChat } from "../src/features/chat/use-chat";
import { savePending, readPending } from "../src/lib/pending";
import { MAX_FILE_SIZE } from "../src/lib/files";
import { json, request, turn, workspace } from "./fixtures";
function wrapper({ children }: { children: ReactNode }) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return <QueryClientProvider client={cache}>{children}</QueryClientProvider>;
}
function server({
  fail = false,
  accepted = false,
  empty = false,
}: { fail?: boolean; accepted?: boolean; empty?: boolean } = {}) {
  let failNext = fail;
  const requests: unknown[] = [];
  const fetch = vi
    .spyOn(globalThis, "fetch")
    .mockImplementation(async (input, init) => {
      const path = String(input).replace("/api/local/", "");
      if (path === "chat/workspaces") return json(empty ? [] : [{ workspace }]);
      if (path === "chat" && init?.method === "POST") {
        const body = JSON.parse(String(init.body));
        requests.push(body);
        if (failNext) {
          failNext = false;
          return json({ error: "Temporary delivery failure" }, 503);
        }
        return json({ ...turn, request: body });
      }
      if (path === "chat") return json([]);
      if (path.endsWith("/activity"))
        return json({ events: [], last_seen_ms: null });
      if (path.startsWith("chat/"))
        return json(accepted ? [{ ...turn, state: "completed" }] : []);
      if (path === "attachments") return json({ ready: true });
      return json({});
    });
  return { fetch, requests };
}
describe("chat recovery and drafts", () => {
  it("reuses the complete request after a failed dispatch and retains session affinity", async () => {
    const s = server({ fail: true });
    const { result } = renderHook(() => useChat(true, true), { wrapper });
    await waitFor(() => expect(result.current.workspaces.isSuccess).toBe(true));
    act(() => result.current.setText("First message"));
    act(() => result.current.send());
    await waitFor(() => expect(result.current.error).toContain("Temporary"));
    const pending = readPending();
    expect(pending?.action.type).toBe("start");
    act(() => result.current.addFiles([new File(["x"], "blocked.txt")]));
    expect(result.current.files).toHaveLength(0);
    act(() => result.current.send());
    await waitFor(() => expect(result.current.pending).toBeNull());
    expect(s.requests[0]).toEqual(s.requests[1]);
    expect(readPending()).toBeNull();
    act(() => result.current.setText("Follow up"));
    act(() => result.current.send());
    await waitFor(() => expect(s.requests).toHaveLength(3));
    expect(s.requests[2]).toMatchObject({
      thread_id: pending?.thread_id,
      action: { type: "send", message: { text: "Follow up" } },
    });
    await waitFor(() => expect(result.current.sending).toBe(false));
    act(() => result.current.end());
    await waitFor(() => expect(s.requests).toHaveLength(4));
    expect(s.requests[3]).toMatchObject({ action: { type: "cancel" } });
  });
  it("reconciles a lost acknowledgement after reload without dispatching again", async () => {
    savePending(request);
    const s = server({ accepted: true });
    const { result } = renderHook(() => useChat(true, false), { wrapper });
    await waitFor(() => expect(result.current.turns.isSuccess).toBe(true));
    await waitFor(() => expect(readPending()).toBeNull());
    expect(result.current.text).toBe("");
    expect(result.current.pending).toBeNull();
    act(() => result.current.setText("A new follow up"));
    expect(result.current.text).toBe("A new follow up");
    expect(s.requests).toHaveLength(0);
    act(() => result.current.navigate(null));
    expect(result.current.thread).toBeNull();
    expect(location.hash).toBe("");
  });
  it("validates attachment count and size, removes drafts, and sends attachment-only messages", async () => {
    const s = server();
    const { result } = renderHook(() => useChat(true, true), { wrapper });
    await waitFor(() => expect(result.current.workspaces.isSuccess).toBe(true));
    act(() =>
      result.current.addFiles([
        new File([new Uint8Array(MAX_FILE_SIZE + 1)], "too-big.bin"),
      ]),
    );
    expect(result.current.error).toContain("8 MiB");
    expect(result.current.files).toHaveLength(0);
    act(() =>
      result.current.addFiles(
        Array.from({ length: 9 }, (_, i) => new File(["a"], `${i}.txt`)),
      ),
    );
    expect(result.current.files).toHaveLength(8);
    expect(result.current.error).toContain("8 files");
    const id = result.current.files[0].id;
    act(() => result.current.removeFile(id));
    expect(result.current.files).toHaveLength(7);
    act(() => result.current.navigate(null));
    act(() => result.current.addFiles([new File(["data"], "data.txt")]));
    act(() => result.current.send());
    await waitFor(() => expect(s.requests).toHaveLength(1));
    expect(s.requests[0]).toMatchObject({
      action: {
        message: {
          text: "Please examine the attached files.",
          attachments: [{ name: "data.txt" }],
        },
      },
    });
  });
  it("rejects invalid programmatic operations before dispatch", async () => {
    const s = server({ empty: true });
    const { result } = renderHook(() => useChat(true, false), { wrapper });
    await waitFor(() => expect(result.current.workspaces.isSuccess).toBe(true));
    act(() => result.current.send());
    await waitFor(() => expect(result.current.error).toContain("workspace"));
    act(() => result.current.end());
    await waitFor(() => expect(result.current.error).toContain("chat first"));
    expect(s.requests).toHaveLength(0);
  });
});
