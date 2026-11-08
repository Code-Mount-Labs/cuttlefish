# Chat, files, and local providers

The current source includes the chat surface and real local provider adapters.
The release gate remains 100%
line, function, and region coverage; passing a live demo does not bypass that gate.

## Setup

1. Install and sign in to Codex or Claude Code on the **execution machine**, under
   the same operating-system account that runs Cuttlefish. Use `codex login` or
   `claude auth login`. Provider credentials remain in that provider's own store.
2. Open the installation dashboard, sign in, and finish database setup.
3. In Settings, check the local provider sign-ins and create a named workspace.
   Choose its provider and whether it may edit files. This configuration is
   immutable; create another workspace for different permissions.
4. On the router dashboard, select that workspace in the chat header. Workspaces
   appear after the execution machine advertises them. The router can live on a
   different computer; it does not run the provider CLI.

Chat requests use the existing authenticated, origin-checked JSON API and the
same routing and durable dispatch machinery as other tasks. A follow-up retains
its node, specialist, and provider-session binding. A supervisor restart does not
restart its specialist. An uncertain provider execution is never automatically
replayed on another machine.

## Interaction

Both dashboards use React and TypeScript, shadcn/ui primitives, and a shared
minimal design system. TanStack Query manages validated JSON data and polling;
component state owns drafts. See [frontend development](../frontend/README.md).
The frontend has its own 100% statement, branch, function, and line coverage gate,
in addition to the unchanged Rust gate. Browser checks use the production CSP.

The installation sidebar keeps **Chats**, **Routines**, **Workflows**, and
**Plugins** in a consistent location. The recent-chat list remains available while
switching sections. On narrow screens, open the navigation menu. Routines own a
prompt, workspace, and schedule; workflows are saved prompts for manual reruns.
See [routines and workflows](routines-and-workflows.md).

The desktop and mobile chat surface includes recent conversations, search, new
chats, a workspace/provider picker, formatted replies, copy controls, file input,
file drag-and-drop and paste, image previews, and downloadable result cards.
Enter sends; Shift+Enter inserts a newline. A pending send retains its exact
request identity through a page reload, so retrying cannot create a second task.
Signing out clears the current browser's pending request and visible chat data.
When reading earlier messages, incoming updates do not pull you away; **Jump to
latest** returns to the bottom. **End session** is in the conversation options
menu after the current task finishes. Ending a session releases its worker;
start a new chat to continue working.

Open **Settings → Password** to change the installation administrator password.
The hosted website has a separate **Account → Password** page. A successful
change clears the browser's authenticated state and asks you to sign in again;
all sessions for that account are revoked. See [password management](local-installation.md#passwords).

Messages and durable activity refresh by polling. The activity timeline reports
preparing, connecting, provider handoff, observed tool use, waiting, and saving
results. Tool labels come from provider events, not timed animations or inferred
percentages. Raw arguments, file contents, and reasoning are never included.
Public provider commentary supplies short descriptions of actual work. Generic
working/waiting transitions are omitted from the timeline; repeated tool stages
are grouped and recent activity is expanded on demand. The UI reports the last
activity time separately from agent connectivity. Silence is not proof of a stall.
Activity continues throughout long turns: the local journal prunes only older,
acknowledged events after retaining the latest 256, and never drops unsent events. Node-local SQLite
retains the outbox until the authenticated router acknowledges the exact sequence
batch. Reopening the chat restores the timeline from the execution PostgreSQL.
Missing heartbeats are shown as interrupted connections, not proof that a task
stopped. Provider turns have no arbitrary ten-minute wall-clock cutoff. If a
provider fails, a bounded explanation is saved without raw transport errors or
private reasoning; already-written files are retained and uncertain effects are
never automatically replayed.

You can send a follow-up while a turn is working. It is saved immediately and
runs in order after the current turn, in the same provider session. Up to 32
unfinished messages may be pending per conversation. This is a durable queue,
not mid-turn interruption. An already-interrupted turn rejects new follow-ups
with an explanation rather than silently placing them in a blocked queue.
Idle sessions remain resumable without consuming active execution capacity.

## Continue an interrupted task

An interrupted turn offers **Inspect saved work**. This asks its original agent
for a read-only checkpoint; it does not invoke a provider or send the original
request again. The review shows the provider, saved session, project file count,
and last available public progress update. Choose **Continue from checkpoint**
to explicitly authorize continuation.

The node verifies its command journal, provider binding, workspace, completed
provider shutdown, and any outstanding approvals or tool results. It hashes the
saved command, binding, configuration, and file contents. An edit between review
and approval invalidates the review. Inspection is bounded to 10,000 entries,
16 directory levels, and 256 MiB of file data; `.git`, `node_modules`, `target`,
and `.venv` are excluded, and symlinks are not traversed. This is a review of the
current files, not a backup or a claim that their contents are correct.

Continuation retains the original command, chat, task, workspace, provider session,
and queued follow-ups. A new fenced worker receives the continuation instructions
in `prompts/execution/recovery.md`, with the original request supplied as context.
It first checks the saved conversation, existing files, artifacts, and delegated
tasks, verifies uncertain effects, and continues only the remaining work. It must
ask when an external effect cannot be verified. The model can still make mistakes;
existing file permissions and approval requirements continue to apply.

Review, approval, and launch intent are durable across server and supervisor
restarts. Repeated clicks and lost responses reuse the same recovery identity.
Recovery never falls back to a fresh provider session. The original agent must
reconnect and support recovery; there is no automatic migration to a different
computer. Nodes opt in with `?recovery=true` on dispatch polling, so
older nodes continue receiving ordinary tasks without unsupported payloads.

Older interruptions may lack a recorded provider shutdown. Missing sessions,
unresolved approvals/tool results, changed files, inaccessible workspaces, and
unknown process state block continuation with an explanation. The saved files
remain intact. Recovery deliberately does not guess that an old process stopped.

Authenticated, origin-checked dashboard API:

- `GET /api/local/chat/{thread}/recovery`: list saved recovery reviews.
- `POST /api/local/chat/{thread}/recovery`: `inspect` with a stable `id` and
  `command_id`, or `continue` with the reviewed `id` and exact `digest`.
- Node dispatch includes an optional `recovery` job; the original authenticated
  node reports its result to `POST /v1/recovery/report`.

## Collaboration

Codex and Claude Code receive three native Cuttlefish tools: discover registered
agents/workspaces, delegate a bounded task, and inspect its result. Tool requests
are journaled before transport, retain their identities across retries, and are
scoped to an authenticated executing parent. Child tasks appear in the parent
chat and can be opened separately. They use the same durable dispatch and
result paths as ordinary chats.

The first version permits top-level delegation only. A child cannot recursively
create more children, and a read-only parent cannot delegate into a workspace
with write permissions. Registered platform/provider metadata does not prove
that an SDK is installed; the child must verify prerequisites. Separate machines
do not implicitly share a filesystem. Use returned artifacts, not assumed paths.

Tool descriptions live in `prompts/collaboration/` and are copied to the
installation's editable prompt directory without overwriting existing edits.
Only these three native tools receive the existing task's collaboration
permission; unrelated MCP integrations remain disabled. No provider credentials
are placed in tool arguments. Upgrade the router and participating agents to the
same release before using these tools.

Token-by-token streaming, active-turn
cancellation, conversation renaming/archiving, voice conversations, and a visual
artifact editor are not implemented. This is a familiar chat interaction model,
not a claim of complete ChatGPT feature parity. An uncertain result requires the operator to inspect the workspace before
recovering it; the activity timeline makes that state visible. Closing the browser does not stop execution.

## Files and capabilities

Each message accepts up to eight files, each at most 8 MiB. Uploads are split into
192 KiB chunks. The execution PostgreSQL database stores immutable metadata and
chunks scoped to the authenticated group. SHA-256 is verified before a file can
be used. Execution nodes download input files into the selected workspace before
accepting the command. Output files go into a command-specific directory, then
into a durable SQLite outbox, then into PostgreSQL before the reply is published.
The hosted account database does not receive these file contents.

The specialist receives small file references instead of base64 file contents.
Images use Codex's native local-image input as well as the file reference.
Specialists must put downloadable results directly in the supplied output
directory. Symlinks, oversized files, excess files, changed upload identities,
and cross-workspace inputs are rejected. Partial uploads are resumable; file
retention and garbage collection require a later storage-management increment.

Capabilities depend on the selected provider and its installed environment:

- Codex can use its available native research and image tools. Live checks must
  verify real cited research, generated images, and image edits, rather than
  accepting a textual claim that a tool ran.
- Document and spreadsheet tasks use the specialist's available tools to create
  actual files. Generated files require independent format/content validation.
- The current Claude Code adapter allows Read, Glob, Grep, Edit, and Write, with
  edit tools disabled for read-only workspaces. It does not currently expose
  Claude web tools or a separate image-generation provider.
- Capability acquisition remains an organization-catalog, human-approved flow.
  A missing provider capability is not permission to install arbitrary software.

The provider's system instructions live in `prompts/execution/specialist.md`.
Installation copies are editable and are preserved during initialization.

## Keeping token overhead bounded

Cuttlefish makes one provider turn per user message. A follow-up resumes the saved
provider session and sends the new message and file references; Cuttlefish does
not re-send its full chat history or activity log. Single-candidate routing avoids
a separate routing model call. The Codex adapter disables unrelated app/plugin
exposure and skill-instruction expansion, and caps ordinary tool output context.
Native provider instructions and the provider's own context management still
apply. Token and billing savings are workload-dependent, not guaranteed.

## API

All installation routes require the local administrator session. Mutations also
require the installation origin. Managed relay access uses the same routes under
`/i/{installation_id}`.

| Route | Purpose |
| --- | --- |
| `GET /api/local/providers` | Installed CLIs and current local sign-in status |
| `GET, POST /api/local/workspaces` | List or create local execution workspaces |
| `GET /api/local/chat/workspaces` | Group workspace directory at the router |
| `GET, POST /api/local/chat` | Recent conversations or idempotent task submission |
| `GET /api/local/chat/{thread_id}` | Most recent 100 turns, in chronological order |
| `GET /api/local/chat/{thread_id}/tasks` | Scoped delegated tasks and destinations |
| `GET /api/local/chat/{thread_id}/activity` | Durable activity and last observed node heartbeat |
| `POST /api/local/attachments` | Idempotent bounded chunk upload |
| `GET /api/local/attachments/{id}/chunks/{index}` | Authenticated chunk download |

The node-only `POST /v1/agent-tools` endpoint authenticates both the node and its executing parent.

Node authentication uses the corresponding `/v1/workspaces` and `/v1/attachments`
routes. Group identifiers alone never authorize access. The hosted platform and
execution database credentials remain separate.

## Checks

Use the standard Rust checks and unchanged coverage command from the README.
The browser contract test uses an isolated profile and a local fixture server;
it needs no provider account, model calls, or production credentials:

```sh
BROWSER_BIN=/absolute/path/to/chromium node tests/web/chat.mjs
BROWSER_BIN=/absolute/path/to/chromium \
  CHAT_TEST_BASE=/i/00000000-0000-0000-0000-000000000123 node tests/web/chat.mjs
```

It checks upload bytes, exact retry identity across a reload, safe Markdown,
responsive layout, and sign-out cleanup. Real provider and relay checks remain
separate because they use subscription access and remote services.

Routine and workflow runs are single-use sessions. After a run finishes, a durable close command releases its worker capacity while retaining the conversation and artifacts. Run the routine or workflow again to start a fresh session. Close-command delivery survives router restarts and never invokes the LLM.
