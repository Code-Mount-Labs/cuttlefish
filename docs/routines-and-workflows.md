# Routines and workflows

The installation sidebar contains **Chats**, **Routines**, **Workflows**, and
**Plugins**. Use Chats for a request or an ongoing conversation, Routines for
scheduled work, and Workflows for a saved task you can run again. Plugins is an
empty section reserved for future integrations.

## Routines

Select **Routines → New routine**. Enter the prompt, choose an existing agent
workspace, set the first run and repeat interval, then give the routine a name.
The routine owns its prompt and workspace; creating a workflow is not required.
The selected workspace determines the provider and file-edit permissions.

The editor offers every 15 minutes, every hour, every 24 hours, and every 7 days.
It also preserves an existing custom interval. The first run uses the browser's
local time zone and is stored as a UTC instant. Repeats use elapsed time rather
than a calendar rule: a 24-hour repeat can move by an hour on the local clock
when daylight-saving time changes. The editor displays the time zone.

Each routine shows its prompt, workspace, active or paused state, interval, and
next occurrence. Use **Edit** to change its definition, **Pause** or **Resume**
to control scheduled occurrences, and **Run now** for an immediate run. Run now
also works while the schedule is paused. Pausing does not cancel work already
queued or dispatched.

Editing only the prompt, name, workspace, or enabled state preserves the next
occurrence. Changing the first run or repeat interval recalculates it from the
new first run. After downtime, missed occurrences are combined into one run.
If a previous run is still pending or executing, the scheduler skips an
overlapping occurrence. Manual Run now requests are separate from this scheduled
overlap check.

## Workflows

Select **Workflows → New workflow**. Enter a prompt, choose an agent workspace,
and give the workflow a name. A prompt can describe several steps for the
specialist; the current editor is not a visual branching workflow builder.
**Run now** creates a fresh conversation with that workspace's provider and
permissions. Workflows have no schedule and do not depend on routines.

## Progress and recovery

Both sections show recent runs with a status and **Open chat** link. The chat
contains the normal activity timeline, provider response, and artifacts. A run
waiting for dispatch stays queued until an agent can accept it. A successful
submission means work was queued, not that its goal is complete.

Each run snapshots the prompt and workspace before dispatch. Later edits cannot
change a queued request. Browser retries retain the run identity, including
after a reload. Each completed run closes its specialist session through a
durable close command, freeing capacity while keeping the transcript and
artifacts. Run the routine or workflow again to start a fresh session.

The router must be running for schedules to dispatch. Definitions and run intents
live in the group's shared execution PostgreSQL database, separate from the
hosted account database. The scheduler locks due definitions and commits the
run intent and next occurrence in one transaction. A restart between dispatch
and acknowledgment retries the same command and thread identities. Close-command
delivery also survives restart and does not invoke the LLM. Uncertain execution
uses the same conservative recovery rules as chat; it is not blindly replayed.

## Updating from Routes

The execution database migration `0009_routines.sql` converts existing scheduled
Routes into independent Routines. It copies each linked workflow's current
prompt and workspace into the routine and preserves the schedule ID, revision,
enabled state, and next occurrence. The original workflow remains available.
Subsequent workflow edits do not change the migrated routine.

Existing run records are linked to the migrated routine without replacing their
saved requests, command IDs, thread IDs, or cleanup IDs. The old scheduled row is
removed so both schedulers cannot dispatch the same migrated schedule. Back up
the execution database before upgrading; do not run an older router binary
against the upgraded schema. An older installation without routine support
shows an update-required message instead of the new editor.

## JSON API

`GET /api/local/automations` returns `workflows`, `routines`, legacy `routes`, and
the latest 100 `runs` across the group. A run identifies its owner with
`workflow_id` or `routine_id`; `route_id` may be present on historical runs.

`POST /api/local/automations` accepts:

| Operation | Body fields |
| --- | --- |
| `save_workflow` | `workflow`: `id`, `revision`, `name`, `workspace_id`, `instructions` |
| `save_routine` | `routine`: the workflow fields plus `enabled`, `first_run_ms`, `interval_minutes` |
| `run` | `workflow_id`, `run_id` |
| `run_routine` | `routine_id`, `run_id` |

Use revision zero to create a definition and the last read revision to update it.
Stale edits are rejected. `next_run_ms` is computed by the server and must not be
included in a save request. Run IDs are caller-generated UUIDs retained across
retries. Reusing a run ID for another routine or workflow is rejected. Schedule
intervals may be between one minute and 365 days. For example:

```json
{
  "type": "save_routine",
  "routine": {
    "id": "00000000-0000-0000-0000-000000000100",
    "revision": 0,
    "name": "Morning briefing",
    "workspace_id": "00000000-0000-0000-0000-000000000005",
    "instructions": "Review project updates and summarize today's priorities.",
    "enabled": true,
    "first_run_ms": 1893499200000,
    "interval_minutes": 1440
  }
}
```

Legacy `save_route` remains available for older API clients, with `route`
containing `id`, `revision`, `name`, `workflow_id`, `enabled`, `first_run_ms`, and
`interval_minutes`. New clients should use independent routines; migrated IDs
cannot be recreated as legacy routes.

All operations require an installation administrator session and a router or
router-plus-agent installation with a ready execution database. Mutations check
the dashboard origin. The authenticated node identity supplies the group scope;
a group ID alone does not authorize access. Managed relay requests use the same
API beneath `/i/{installation_id}`.
