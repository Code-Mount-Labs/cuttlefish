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
    offline = !done && lastSeen !== null && now - lastSeen > 20000;
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
    hint: attention
      ? "Execution stopped with an uncertain result. Check the agent before retrying; this task won’t be replayed automatically."
      : offline
        ? "The task may still be running. Updates will resume when the agent reconnects."
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
