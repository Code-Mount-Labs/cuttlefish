# Editable model instructions and harnesses

All application-owned LLM wording belongs here, not in Rust string literals.

| File | Purpose | Loaded by |
| --- | --- | --- |
| `branding/logo.md` | Original logo-generation instructions (design time only) | Image generation; never sent by the runtime |
| `router/selection.md` | Placement only; no execution authority | Routing service |
| `super-agent/planner.md` | Planning, delegation, verification and native learning | Node supervisor |
| `capabilities/<name>/instructions.md` | Versioned capability harness | Organization catalog publication |
| `capabilities/<name>/recipe.md` | Exact acquisition and permission proposal | Organization catalog publication |
| `capabilities/<name>/evaluation.md` | Tests and activation criteria | Organization catalog publication |

The `example` capability is a template, not an installed tool or working provider.
Copy it under a meaningful name and replace its contents before publishing.

## Editing and loading

The router accepts `serve --prompt-dir /absolute/path/to/prompts` when a model is
configured. The super-agent settings file accepts
`"prompt_dir": "/absolute/path/to/prompts"`. Paths are resolved from that directory
using the table above. Files are read once at startup; restart the relevant process
to apply edits. A running process retains its reviewed prompt for its lifetime.
Invalid, missing, empty, non-UTF-8 or oversized overrides fail startup. Each prompt
is limited to 64 KiB. There is no fallback when an explicit override is invalid.
Without an override, binaries use the Markdown snapshot bundled at build time.

The catalog command accepts a `manifest.json` with `recipe_file`,
`instructions_file` and `evaluation_file`, resolved relative to the manifest.
It reads the Markdown and publishes an immutable PostgreSQL snapshot. To change
a published recipe or harness, assign a new `release_id` and set `supersedes` to
the prior release. Existing approvals do not transfer to the new version.

## What stays outside this folder

User requests, peer messages, execution results and catalog snapshots are runtime
data stored in the journal/database and serialized into model context. API envelopes,
JSON output schemas and validation remain in Rust. These are protocol, not editable
wording. Do not put credentials in prompts or catalog files.

Prompt edits cannot grant approval, bypass group membership, rewrite dispatched
work, or relax lifecycle rules: those checks run in Rust and database transactions.
Review prompt diffs and run the test suite after edits. HTTP tests verify custom
prompt loading; mocked model tests are not a measure of real model decision quality.
