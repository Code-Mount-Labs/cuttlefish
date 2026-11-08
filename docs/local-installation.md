# Local installation and database setup

The Rust `cuttlefish` binary runs a local dashboard before PostgreSQL exists. It
supports router + agent, router only, and agent only. A group has exactly one
registered router; agent-only enrollment requires that router to be ready.

## Install a release

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/Code-Mount-Labs/cuttlefish/v0.2.0-alpha.6/install.sh | sh
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
Cuttlefish runs in the foreground by default. On macOS, the optional login
service below runs it in the background. The managed relay connector has its
own native OS service and survives supervisor restart.

## macOS background startup

After completing setup, stop the foreground Cuttlefish process with Ctrl+C, then
install a service as your normal macOS account (do not use sudo):

```sh
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service install --listen 127.0.0.1:8080
```

This starts the existing installation now and after you log in to your Mac.
You can close the terminal. It uses the same state directory, database, provider
sign-ins, and managed address. It does not run before login, while logged out,
or while the Mac is asleep. A router that must always be available belongs on
an always-on machine.

```sh
# Check the service and dashboard
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service status

# Restart after updating the application binaries
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service restart

# Stop now and disable login startup until service start is run
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service stop

# Start and re-enable login startup
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service start

# Remove this startup definition, preserving all application data
"$HOME/.local/share/cuttlefish/bin/cuttlefish" --state-dir "$HOME/.cuttlefish" service uninstall
```

Each canonical state directory gets a distinct LaunchAgent in
`~/Library/LaunchAgents/com.cuttlefish.app.<state-hash>.plist`. Startup commands
use the listener saved at installation, including custom ports. To change it,
run `service install --listen <address>` again. The installer refuses to take
over a port already occupied by a foreground or unrelated process.

Logs are private files in `<state-dir>/service/stdout.log` and `stderr.log`.
The service preserves the installing account's `HOME`, `PATH`, and configured
`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `DOCKER_CONFIG`, and `XDG_CONFIG_HOME` paths.
It does not copy arbitrary environment variables or API keys into its definition.
Configure model keys through the local dashboard. Changing provider/config paths
later requires running `service install` again from the intended environment.
Managed PostgreSQL recovery uses the existing Docker startup path; Docker
Desktop may still require its own first-run authorization.

The service retries crash recovery through launchd. Removing it does not remove
PostgreSQL, the relay connector, provider installations, journals, or workspaces.
Existing specialist processes retain their independent lifetime across a
supervisor restart.

## Passwords

The hosted website account and each installation's administrator account are
independent. On an installation dashboard, open **Settings → Password**. On the
hosted website, open **Account → Password**. Enter the current password, a new
password, and its confirmation. Passwords must contain 12–1024 UTF-8 bytes; the
same policy applies to initial setup and sign-in.

A successful change signs that account out on all devices, including the current
browser. Sign in again with the new password. Changing one account does not
change another installation's password, provider sign-ins, database credentials,
or model keys. It does not cancel running agent tasks.

The server verifies the current password and stores a fresh Argon2 hash.
Replacement and session revocation commit atomically; failed writes preserve the
previous password and sessions. Concurrent changes using the old password cannot
both succeed, and a sign-in that started with the old password cannot create a
surviving session after the change. The replacement and revocations survive a
restart. This is a signed-in password change, not a forgotten-password reset.

Both endpoints accept JSON `current_password` and `new_password`, return
`{"ok":true}`, and expire the session cookie:

| Account | Endpoint |
| --- | --- |
| Installation administrator | `POST /api/local/password` |
| Hosted website | `POST /api/v1/auth/password` |

Browser requests require the authenticated session and matching Origin. Use the
installation endpoint through its own dashboard or managed relay prefix; the
hosted endpoint belongs to the website. Never send an installation password to
the hosted account endpoint.

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
| POST | `/api/local/password` | Verify current password, replace it, and revoke all local sessions |
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
password changes, atomic session revocation, concurrent sign-in/change races,
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
