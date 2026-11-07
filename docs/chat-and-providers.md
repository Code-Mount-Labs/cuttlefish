# Chat, files, and local providers

This increment adds the chat surface and real local provider adapters. It is not
included in the published `v0.2.0-alpha.2` installer. The release gate remains 100%
line, function, and region coverage; passing a live demo does not bypass that gate.

## Setup

1. Install and sign in to Codex or Claude Code on the **execution machine**, under
   the same operating-system account that runs Cuttlefish. Use `codex login` or
   `claude auth login`. Provider credentials remain in that provider's own store.
2. Open the installation dashboard, sign in, and finish database setup.
3. In Settings, check the local provider sign-ins and create a named workspace.
   Choose its provider and whether it may edit files. This configuration is
   immutable; create another workspace for different permissions.
4. On the router dashboard, select that workspace in **Chat & tasks**. Workspaces
   appear after the execution machine advertises them. The router can live on a
   different computer; it does not run the provider CLI.

Chat requests use the existing authenticated, origin-checked JSON API and the
same routing and durable dispatch machinery as other tasks. A follow-up retains
its node, specialist, and provider-session binding. A supervisor restart does not
restart its specialist. An uncertain provider execution is never automatically
replayed on another machine.

## Interaction

The desktop and mobile chat surface includes recent conversations, search, new
chats, a workspace/provider picker, formatted replies, copy controls, file input,
file drag-and-drop and paste, image previews, and downloadable result cards.
Enter sends; Shift+Enter inserts a newline. A pending send retains its exact
request identity through a page reload, so retrying cannot create a second task.
Signing out clears the current browser's pending request and visible chat data.

Messages currently refresh by polling. Token-by-token streaming, active-turn
cancellation, conversation renaming/archiving, voice conversations, and a visual
artifact editor are not implemented. This is a familiar chat interaction model,
not a claim of complete ChatGPT feature parity. A pending result remains pending
if the worker becomes uncertain; the operator must inspect its journal before
recovering it. Closing the browser does not stop execution.

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
| `POST /api/local/attachments` | Idempotent bounded chunk upload |
| `GET /api/local/attachments/{id}/chunks/{index}` | Authenticated chunk download |

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
