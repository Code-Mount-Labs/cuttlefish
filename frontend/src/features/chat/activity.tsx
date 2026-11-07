import { Check, LoaderCircle, TriangleAlert } from "lucide-react";
import type { ActivityEvent, Turn } from "../../lib/contracts";
import { activityLabel, activityState } from "../../lib/activity";
export function TaskActivity({
  turn,
  events,
  lastSeen,
}: {
  turn: Turn;
  events: ActivityEvent[];
  lastSeen: number | null;
}) {
  const state = activityState(turn, events, lastSeen);
  return (
    <section
      className={`task-activity ${state.tone}`}
      aria-label="Task activity"
      aria-live="off"
    >
      <details>
        <summary>
          {state.active ? (
            <LoaderCircle className="spinning" size={15} />
          ) : state.tone === "complete" ? (
            <Check size={15} />
          ) : (
            <TriangleAlert size={15} />
          )}
          <span className="activity-title">{state.title}</span>
          <span className="activity-meta">
            {events.length > 0
              ? `${events.length} ${events.length === 1 ? "update" : "updates"}`
              : ""}
          </span>
        </summary>
        <ol className="activity-timeline">
          {events.map((event) => (
            <li key={event.sequence}>
              <span>
                {activityLabel(event.phase, event.provider).replace(/…$/, "")}
              </span>
              <time dateTime={new Date(event.at_ms).toISOString()}>
                {new Date(event.at_ms).toLocaleTimeString([], {
                  hour: "2-digit",
                  minute: "2-digit",
                  second: "2-digit",
                })}
              </time>
            </li>
          ))}
          {!events.length && <li>Detailed activity is not available yet.</li>}
        </ol>
      </details>
      {state.hint && state.tone !== "active" && (
        <p className="activity-hint">{state.hint}</p>
      )}
    </section>
  );
}
