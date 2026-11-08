import { useEffect, useState } from "react";
import {
  Check,
  ChevronDown,
  Clock3,
  LoaderCircle,
  TriangleAlert,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ActivityEvent, Turn } from "../../lib/contracts";
import {
  activityLabel,
  activityState,
  activitySteps,
} from "../../lib/activity";
export function TaskActivity({
  turn,
  events,
  lastSeen,
}: {
  turn: Turn;
  events: ActivityEvent[];
  lastSeen: number | null;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (turn.state === "completed") return;
    const timer = setInterval(() => setNow(Date.now()), 5000);
    return () => clearInterval(timer);
  }, [turn.state]);
  const state = activityState(turn, events, lastSeen, now);
  const [expanded, setExpanded] = useState(false);
  const steps = activitySteps(events);
  const visible = expanded ? steps : steps.slice(-6);
  const update = events.filter((event) => !!event.summary).at(-1);
  const queued = turn.state === "queued" && !events.length;
  return (
    <section
      className={`task-activity ${state.tone}`}
      aria-label="Task activity"
      aria-live="off"
    >
      <details>
        <summary>
          {queued ? (
            <Clock3 size={15} />
          ) : state.active ? (
            <LoaderCircle className="spinning" size={15} />
          ) : state.tone === "complete" ? (
            <Check size={15} />
          ) : (
            <TriangleAlert size={15} />
          )}
          <span className="activity-title">
            {queued && turn.request.action.type === "send"
              ? "Queued · follows the current turn"
              : state.title}
          </span>
          <span className="activity-meta">View activity</span>
          <ChevronDown className="activity-chevron" size={14} />
        </summary>
        <ol className="activity-timeline">
          {visible.map((event) => (
            <li key={event.sequence}>
              <span>
                {event.summary ||
                  activityLabel(event.phase, event.provider).replace(/…$/, "")}
                {event.count > 1 && (
                  <span className="activity-count"> ×{event.count}</span>
                )}
              </span>
              <time dateTime={new Date(event.at_ms).toISOString()}>
                {new Date(event.at_ms).toLocaleTimeString([], {
                  hour: "2-digit",
                  minute: "2-digit",
                })}
              </time>
            </li>
          ))}
          {!steps.length && (
            <li>
              {queued
                ? "Your message is saved. It will be delivered in order."
                : "The agent has not shared detailed activity yet."}
            </li>
          )}
        </ol>
        {steps.length > 6 && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setExpanded(!expanded)}
          >
            {expanded
              ? "Show recent activity"
              : `Show all ${steps.length} activities`}
          </Button>
        )}
      </details>
      {state.active && update && (
        <p className="activity-update">{update.summary}</p>
      )}
      {state.active && (
        <p className="activity-freshness">
          {state.lastUpdate}
          {lastSeen !== null && " · Agent connected"}
        </p>
      )}
      {state.hint && (state.tone !== "active" || state.quiet) && (
        <p className="activity-hint" role={state.quiet ? "status" : undefined}>
          {state.hint}
        </p>
      )}
    </section>
  );
}
