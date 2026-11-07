import { describe, it, expect, vi } from "vitest";
import { z } from "zod";
import {
  ApiError,
  errorMessage,
  local,
  relayBase,
  request as api,
  safeWebUrl,
} from "../src/lib/api";
import {
  activityLabel,
  activityState,
  providerLabel,
} from "../src/lib/activity";
import { phaseSchema } from "../src/lib/contracts";
import { readPending, savePending } from "../src/lib/pending";
import { createObjectUrlStore } from "../src/hooks/use-object-url";
import { allPages, installCommand } from "../src/features/platform/api";
import { json, request, turn } from "./fixtures";
describe("typed API boundary", () => {
  it("keeps relay requests same-origin and validates response schemas", async () => {
    history.replaceState(null, "", "/i/00000000-0000-0000-0000-000000000123/");
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async () => json({ ok: true }));
    const controller = new AbortController();
    expect(
      await local("config", z.object({ ok: z.boolean() }), {
        signal: controller.signal,
      }),
    ).toEqual({ ok: true });
    expect(fetch).toHaveBeenCalledWith(
      "/i/00000000-0000-0000-0000-000000000123/api/local/config",
      expect.objectContaining({
        method: "GET",
        credentials: "same-origin",
        signal: controller.signal,
      }),
    );
    await local("chat", z.unknown(), { body: { text: "hello" } });
    expect(fetch).toHaveBeenLastCalledWith(
      expect.anything(),
      expect.objectContaining({ method: "POST", body: '{"text":"hello"}' }),
    );
    expect(relayBase("/")).toBe("");
    expect(relayBase("/i/not-an-id")).toBe("");
  });
  it("rejects malformed replies and preserves error status", async () => {
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(new Response("broken", { status: 502 }));
    await expect(api("/test", z.unknown())).rejects.toThrow(
      "unexpected response",
    );
    fetch.mockImplementation(async () => json({ error: "Sign in" }, 401));
    await expect(api("/test", z.unknown())).rejects.toMatchObject({
      status: 401,
    });
    fetch.mockImplementation(async () => json({}, 403));
    await expect(api("/test", z.unknown())).rejects.toThrow("403");
    fetch.mockImplementation(async () => json({ ok: "wrong" }));
    await expect(
      api("/test", z.object({ ok: z.boolean() })),
    ).rejects.toBeInstanceOf(ApiError);
    expect(errorMessage(new Error("detail"))).toBe("detail");
    expect(errorMessage(null)).toContain("try again");
  });
  it("allows only explicit web schemes", () => {
    expect(safeWebUrl("https://example.com")).toBe("https://example.com/");
    expect(safeWebUrl("http://example.com", true)).toBeUndefined();
    expect(safeWebUrl("javascript:alert(1)")).toBeUndefined();
    expect(safeWebUrl(null)).toBeUndefined();
    expect(safeWebUrl("http://example.com")).toBe("http://example.com/");
  });
});
describe("durable requests", () => {
  it("round trips exact identities and rejects corrupt browser state", () => {
    expect(readPending()).toBeNull();
    savePending(request);
    expect(readPending()).toEqual(request);
    savePending(null);
    expect(readPending()).toBeNull();
    sessionStorage.setItem("cuttlefish:pending:", "bad");
    expect(readPending()).toBeNull();
    sessionStorage.setItem("cuttlefish:pending:", "{}");
    expect(readPending()).toBeNull();
  });
});
describe("factual activity", () => {
  it("labels every observed phase and provider", () => {
    for (const phase of phaseSchema.options)
      expect(activityLabel(phase, "claude").length).toBeGreaterThan(5);
    expect(providerLabel("codex")).toBe("Codex");
    expect(providerLabel("claude")).toBe("Claude Code");
  });
  it("distinguishes queued, active, interrupted, uncertain and complete", () => {
    const event = {
      command_id: request.command_id,
      sequence: 1,
      provider: "claude" as const,
      phase: "reading_files" as const,
      at_ms: 1000,
    };
    expect(activityState(turn, [event], 1000, 2000).title).toBe(
      "Claude Code is reading files…",
    );
    expect(activityState(turn, [], null).title).toContain("Handed");
    expect(
      activityState({ ...turn, state: "queued" }, [], null).title,
    ).toContain("available");
    expect(activityState(turn, [], 1000, 30000).tone).toBe("offline");
    expect(
      activityState(turn, [{ ...event, phase: "needs_attention" }], 1000, 30000)
        .tone,
    ).toBe("attention");
    expect(
      activityState({ ...turn, state: "completed" }, [event], 1000, 30000),
    ).toMatchObject({
      title: "Completed",
      active: false,
      hint: "",
      tone: "complete",
    });
    expect(
      activityState(
        {
          ...turn,
          state: "completed",
          outcome: { type: "rejected", reason: "Unavailable" },
        },
        [],
        null,
      ).title,
    ).toContain("couldn’t");
  });
});
describe("object URL lifecycle", () => {
  it("releases URLs only after the last subscriber and reacquires on remount", () => {
    const store = createObjectUrlStore(new Blob(["image"])),
      notify = vi.fn();
    expect(store.snapshot()).toBeUndefined();
    const remove = store.subscribe(notify),
      other = store.subscribe(vi.fn());
    expect(store.snapshot()).toBe("blob:preview");
    expect(notify).toHaveBeenCalledOnce();
    remove();
    expect(store.snapshot()).toBe("blob:preview");
    other();
    expect(store.snapshot()).toBeUndefined();
    store.subscribe(notify)();
    expect(store.snapshot()).toBeUndefined();
    const empty = createObjectUrlStore(undefined);
    empty.subscribe(notify)();
    expect(empty.snapshot()).toBeUndefined();
  });
});
describe("platform pagination", () => {
  it("collects every page and rejects nonadvancing cursors", async () => {
    const rows = Array.from({ length: 100 }, (_, i) => ({
      id: String(i + 1).padStart(36, "0"),
    }));
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(json(rows))
      .mockResolvedValueOnce(json([{ id: "last" }]));
    expect(
      await allPages("/groups", z.object({ id: z.string() }), (r) => r.id),
    ).toHaveLength(101);
    expect(fetch).toHaveBeenCalledTimes(2);
    fetch.mockImplementation(async () => json(rows));
    await expect(
      allPages("/groups", z.object({ id: z.string() }), (r) => r.id),
    ).rejects.toThrow("Invalid page");
  });
  it("quotes installation origin", () => {
    expect(installCommand("https://example.com")).toContain(
      "--platform 'https://example.com'",
    );
    expect(installCommand("a'b")).toContain("'a'\"'\"'b'");
  });
});
