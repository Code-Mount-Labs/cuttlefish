# Local installation and database setup

The Rust `cuttlefish` binary runs a local dashboard before PostgreSQL exists. It
supports router + agent, router only, and agent only. A group has exactly one
registered router; agent-only enrollment requires that router to be ready.

## Install a release

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/Code-Mount-Labs/cuttlefish/v0.2.0-alpha.4/install.sh | sh
```

Linux and macOS releases include ARM64 and x86-64 binaries. Windows installation
is deferred pending testing on a suitable Windows 11 machine.
`CUTTLEFISH_INSTALL_DIR` selects the installation directory;
`CUTTLEFISH_NO_SETUP=1` installs binaries without starting interactive setup.

## Build from source

```sh
cargo build --locked -p cuttlefish-machine -p orchestrator-node --bins
./target/debug/cuttlefish setup
```

Setup asks for the group ID, a one-use invitation from the hosted dashboard, a
role, a machine name, and remote access: local-only, managed HTTPS, or an
existing HTTPS address. Managed access uses the Cuttlefish relay with an outbound
connection; no personal domain or Cloudflare account is needed. `--listen`
controls the local bind address, which defaults to loopback. An existing HTTPS
address requires your own reverse proxy or tunnel. See [remote access](remote-access.md).

The terminal prints a one-time local setup code. Open the dashboard, enter that
code, and create the local administrator. The hosted account and this local
administrator are independent. The default state directory is `~/.cuttlefish`;
use `--state-dir` to override it. To resume, run `cuttlefish serve` with the same
state directory and listener settings.

The shell bootstrap downloads versioned, checksum-verified release archives.
Windows scripts remain experimental and are not a supported installation path.
The main Cuttlefish process runs in the foreground; automatic startup for that
process is not yet implemented. The managed relay connector has its own native
OS service and survives supervisor restart.

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
**Ubuntu and Debian**, and Docker Desktop on **macOS**. Existing
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

Run only against a dedicated test PostgreSQL database. The required gate is
100% line, function, and region coverage, including the Docker tests. Fault tests
disconnect a real PostgreSQL connection at every provisioning transaction step
and verify that no partially created application role survives.

`tests/install/fresh-linux.sh` creates a disposable Lima VM with no host mounts
or preinstalled Docker. It tests initial installation, repeat installation, and
daemon/data persistence through a full VM reboot. Ubuntu 24.04 and Debian 13
are exercised in both root and rootless modes. An optional third argument accepts
a release archive and also exercises the bootstrap and complete local setup flow.
`tests/install/release-smoke.py` uses a local directory fixture while exercising
real authentication, provisioning, agent enrollment, and supervisor restarts.
