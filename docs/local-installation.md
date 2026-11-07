# Local installation and database setup

The Rust `cuttlefish` binary runs a local dashboard before PostgreSQL exists. It
supports router + agent, router only, and agent only. A group has exactly one
registered router; agent-only enrollment requires that router to be ready.

## Current development entry point

```sh
cargo build --locked -p cuttlefish-machine -p orchestrator-node --bins
./target/debug/cuttlefish setup
```

Setup asks for the group ID, a one-use invitation from the hosted dashboard, a
role, a machine name, and its reachable dashboard/router origin. The default
origin binds to loopback. For remote access, supply a reachable HTTPS origin and
configure a reverse proxy or private tunnel. `--listen` controls the local bind
address. Registration never opens laptop ports automatically.

The terminal prints a one-time local setup code. Open the dashboard, enter that
code, and create the local administrator. The hosted account and this local
administrator are independent. The default state directory is `~/.cuttlefish`;
use `--state-dir` to override it. To resume, run `cuttlefish serve` with the same
state directory and listener settings.

Public bootstrap scripts (`install.sh`, `install.ps1`) are prepared for versioned,
checksum-verified release archives. They are not usable until those release
assets are published. No installation release has been published for this change.
The current service runs in the foreground; OS service registration and automatic
startup at login or boot are not yet implemented.

## Database choices

- **Create PostgreSQL here:** save a stable provisioning plan, install/start Docker
  if necessary, create an owned persistent volume and loopback-only container,
  create a non-superuser application role, then verify and migrate the database.
- **Use existing PostgreSQL:** accept the connection URL only on this local
  dashboard, verify a writable primary, and apply execution migrations. A failed
  connection can be corrected. An activated primary cannot be silently replaced.
- **Set up later:** keep the dashboard and local login available. Configure the
  database later through the same UI. Execution remains unavailable until ready.

Provisioning is independent of the browser request. Closing the page does not
cancel installation. On service restart, a previously ready database is checked
again before routing becomes available. Interrupted work reuses the saved
container, volume, and secrets. Failed work remains visible until the operator
retries; it does not create replacement data volumes automatically.

Docker installation currently supports official Docker Engine packages on
**Ubuntu and Debian**, and Docker Desktop on **macOS and Windows**. Existing
working Docker is reused. Linux non-root installation uses rootless Docker rather
than adding the application account to the root-equivalent Docker group. Package
installation still needs administrator privileges. The dashboard never collects
an OS administrator password: run `cuttlefish install-docker` interactively when
required, complete Docker Desktop's own first-run steps, then retry in the UI.
Other Linux distributions require a manually installed Docker Engine for now.

The managed image is pinned by digest. PostgreSQL 18 stores data in the persistent
volume mounted at `/var/lib/postgresql`. The container restarts unless explicitly
stopped. Cuttlefish verifies ownership labels, image, port binding, mount, restart
policy, and bootstrap settings before adopting a container. It never removes an
unrelated container or volume to make setup succeed.

## Credentials and sharing

The hosted platform receives enrollment credentials and directory heartbeats,
never user database URLs or model keys. Agents authenticate to their group's
router; they do not receive PostgreSQL administrator credentials. Each agent
retains its own SQLite execution journal.

The local SQLite setup journal and generated bootstrap files contain secrets and
must be protected as credentials. Unix state directories use mode 0700 and secret
files 0600. These are file permissions, not encryption at rest. Back up this state
and the PostgreSQL volume securely. Automated backup, restore, secret rotation,
primary migration, and read-replica management are not part of this increment.
No Redis service is required.

## Local JSON API

Browser mutations require the exact configured Origin and an authenticated local
session (except initial setup/login). Sessions use HttpOnly, SameSite=Strict
cookies, Secure on HTTPS, and expire after eight hours. Authentication and failed
login reservations survive restart. JSON inputs have a 16 KiB limit.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/local/status` | Whether a local administrator exists |
| POST | `/api/local/setup` | One-time code, username, password |
| POST | `/api/local/login` | Local administrator login |
| POST | `/api/local/logout` | Revoke the current session |
| GET | `/api/local/config` | Identity, safe database status, model name/endpoint |
| POST | `/api/local/database` | Queue `managed`, `existing`, or `later` selection; 202 |
| POST | `/api/local/database/retry` | Queue the exact saved failed plan; 202 |
| POST | `/api/local/model` | Store local endpoint, model and API key |
| POST | `/api/node/enroll` | Enroll a directory-authorized group agent |
| * | `/v1/*` | Existing separately authenticated execution/router APIs |

The local dashboard never returns database passwords or model keys. A known
agent may reconnect while the hosted directory is unavailable. New enrollment
still requires group-scoped directory authorization; revoked execution
credentials cannot use the reconnect path.

Editable prompts are created under the state directory's `prompts/` folder.
Existing edits are preserved. Routing loads `prompts/router/selection.md`; invalid
or missing instructions fail closed rather than silently reverting to defaults.

## Validation

Tests cover deferred setup, persisted administrator sessions and rate limits,
interrupted provisioning, immutable resource identity, ownership conflicts,
Docker reuse/installation boundaries, bounded subprocess output, group enrollment,
revocation, separate registry/execution schemas, and whole-service restart.

The optional real Docker test creates only uniquely named test resources, checks
that the application role is not a superuser, restarts the container and local
service, verifies stored data, and removes its own container and volume:

```sh
COORDINATOR_TEST_DATABASE_URL=... cargo test -p cuttlefish-machine --features docker-tests
```

Run only against a dedicated test PostgreSQL database. The workspace still targets
100% line, function, and region coverage. The new installation layer has not yet
reached that gate; release publication remains pending. Tests on this workstation
do not establish that a fresh Windows or Linux OS installation has been validated.
