import type { ActivityEvent, Phase, Provider, Turn } from "./contracts";
export const providerLabel = (provider: Provider) =>
  provider === "claude" ? "Claude Code" : "Codex";
export function activityLabel(phase: Phase, provider: Provider) {
  const name = providerLabel(provider);
  const labels: Record<Phase, string> = {
    preparing: "Preparing task…",
    connecting: `Connecting to ${name}…`,
    submitted: `Passed task to ${name}`,
    working: `${name} is working…`,
    commentary: `${name} shared an update`,
    reading_files: `${name} is reading files…`,
    writing_files: `${name} is updating files…`,
    running_command: `${name} is running a command…`,
    searching_web: `${name} is searching the web…`,
    generating_image: `${name} is creating an image…`,
    delegating: `${name} is coordinating sub-agents…`,
    using_tool: `${name} is using a tool…`,
    waiting: `Waiting for ${name}…`,
    collecting_results: "Saving results…",
    needs_attention: "This task needs attention",
  };
  return labels[phase];
}
export function activityState(
  turn: Turn,
  events: ActivityEvent[],
  lastSeen: number | null,
  now = Date.now(),
) {
  const latest = events.at(-1),
    done = turn.state === "completed",
    failed = turn.outcome?.type === "rejected",
    attention = !done && latest?.phase === "needs_attention",
    offline = !done && lastSeen !== null && now - lastSeen > 20000,
    quiet = !done && !!latest && now - latest.at_ms > 60000;
  return {
    title: done
      ? failed
        ? "Task couldn’t be completed"
        : "Completed"
      : attention
        ? "This task needs attention"
        : offline
          ? "Agent connection interrupted"
          : latest
            ? activityLabel(latest.phase, latest.provider)
            : turn.state === "queued"
              ? "Waiting for an available agent…"
              : "Handed to your agent · waiting for an update…",
    lastUpdate: latest
      ? `Last activity ${elapsed(now - latest.at_ms)} ago`
      : "Waiting for the first update",
    quiet,
    hint: attention
      ? `${latest?.summary || "The provider stopped before confirming a result."} Work already saved has been kept. Review the workspace before starting a new session; this turn won’t be repeated automatically.`
      : offline
        ? "The task may still be running. Updates will resume when the agent reconnects."
        : quiet
          ? "No new provider activity for over a minute. Silence does not confirm whether the task is progressing. You can send a follow-up; it will run after the current turn."
          : done
            ? ""
            : "You can leave this chat and return. Your progress is saved.",
    active: !done && !attention && !offline,
    tone:
      failed || attention
        ? "attention"
        : offline
          ? "offline"
          : done
            ? "complete"
            : "active",
  };
}

export function elapsed(ms: number) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return seconds < 60
    ? `${seconds}s`
    : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
export function activitySteps(events: ActivityEvent[]) {
  const meaningful = events.filter(
    (event, index) =>
      !["working", "waiting"].includes(event.phase) ||
      (event.phase === "working" &&
        (index === 0 || event.provider !== events[index - 1].provider)),
  );
  return meaningful.reduce<(ActivityEvent & { count: number })[]>(
    (steps, event) => {
      const prior = steps.at(-1);
      if (
        prior &&
        prior.phase === event.phase &&
        prior.summary === event.summary &&
        prior.provider === event.provider
      ) {
        prior.count++;
        prior.at_ms = event.at_ms;
      } else steps.push({ ...event, count: 1 });
      return steps;
    },
    [],
  );
}
