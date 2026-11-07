# Routes and workflows

The workspace has a primary navigation rail for Chats, Routes, Workflows, and Plugins. Chats continue to support one-off requests and follow-ups. Plugins is an empty section reserved for future integrations.

## Workflows

Create a workflow with a name, an existing agent workspace, and instructions. Instructions can describe a sequence of tasks for the specialist. Run starts a fresh conversation using the workspace's configured provider and permissions. Each run stores a snapshot of its request, so later edits do not change work already queued.

The run history links to the normal chat transcript, including progress, provider replies, and artifacts. A run waiting for dispatch displays its status until the router can assign it. Manual retries retain the same run identity, including after a browser reload.

## Routes

Create a workflow first, then select it in a route. Choose a first occurrence and an hourly, 24-hour, or 7-day repeat. The first occurrence uses the browser's local time zone and is stored as a UTC instant. Repeats use elapsed time. A 24-hour repeat can move by an hour on the local clock when daylight-saving time changes.

Routes show their next occurrence and can be edited, paused, or resumed. Pausing prevents future occurrences. Work already queued or dispatched keeps its existing state. After downtime, missed occurrences are combined into one run. If a route's previous run is still pending or executing, an overlapping occurrence is skipped.

The router must be running for schedules to dispatch. Workflows, route definitions, and run intents live in the group's shared PostgreSQL database. The scheduler locks due route rows and commits the run intent and next occurrence together. Dispatch uses the existing idempotent task router. A restart between dispatch and acknowledgement retries the same command and thread identities.

## JSON API

The installation dashboard exposes `GET /api/local/automations` for workflows, routes, and the latest 100 runs. `POST /api/local/automations` accepts one of these operations.

- `save_workflow` with `workflow` containing `id`, `revision`, `name`, `workspace_id`, and `instructions`.
- `save_route` with `route` containing `id`, `revision`, `name`, `workflow_id`, `enabled`, `first_run_ms`, and `interval_minutes`.
- `run` with `workflow_id` and a caller-generated `run_id` retained for retries.

Use revision zero to create a definition. Updates must supply the last read revision. Stale edits are rejected. Schedule intervals may be between one minute and 365 days. All endpoints require the installation administrator session. Mutations enforce the dashboard origin, and only installations with the router role expose these operations. The authenticated node identity determines the group scope.
