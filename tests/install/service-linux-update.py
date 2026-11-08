#!/usr/bin/env python3
"""Exercise the real binary, installer and detached updater under Linux systemd.

Run only on a disposable Linux host:
  service-linux-update.py <binary-directory> <empty-test-directory>
Only the release download transport and group directory are fixtures. Both
supervisors, HTTP authentication, SQLite, checksum verification, atomic release
activation, CLI update and restart recovery are real.
"""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tarfile
import uuid

TESTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "release_smoke", TESTS / "release-smoke.py"
)
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
binaries = Path(sys.argv[1]).resolve()
root = Path(sys.argv[2]).resolve()
root.mkdir(parents=True, exist_ok=False)
version = subprocess.check_output(
    [str(binaries / "cuttlefish"), "--version"], text=True
).split()[1]
tag = "v" + version
arch = "aarch64" if platform.machine() == "aarch64" else "x86_64"
asset_name = f"cuttlefish-{tag}-{arch}-unknown-linux-musl.tar.gz"
asset = root / asset_name
package = root / "package"
package.mkdir()
for name in ["cuttlefish", "orchestrator-node", "orchestrator-coordinator"]:
    shutil.copy2(binaries / name, package / name)
    subprocess.run(["strip", str(package / name)], check=True)
with tarfile.open(asset, "w:gz") as archive:
    for path in package.iterdir():
        archive.add(path, arcname=path.name)
(root / "SHA256SUMS").write_text(
    hashlib.sha256(asset.read_bytes()).hexdigest() + "  " + asset_name + "\n"
)
transport = root / "transport"
transport.mkdir()
curl = transport / "curl"
allowed = f"https://github.com/Code-Mount-Labs/cuttlefish/releases/download/{tag}/"
curl.write_text(f"""#!{sys.executable}
import os,shutil,sys
from pathlib import Path
args=sys.argv[1:]
urls=[arg for arg in args if arg.startswith({allowed!r})]
if urls:
 name=urls[0].rsplit('/',1)[1]
 assert name in {[asset_name, "SHA256SUMS"]!r}
 shutil.copyfile(Path({str(root)!r})/name,args[args.index('-o')+1])
else:os.execv('/usr/bin/curl',['/usr/bin/curl',*args])
""")
curl.chmod(0o700)
os.environ["PATH"] = str(transport) + os.pathsep + os.environ["PATH"]
provider = root / "private-provider"
provider.mkdir()
(provider / "auth.json").write_text("isolated provider credential marker")
os.environ["CODEX_HOME"] = str(provider)
app = root / "app"
env = dict(
    os.environ,
    CUTTLEFISH_INSTALL_DIR=str(app),
    CUTTLEFISH_VERSION=tag,
    CUTTLEFISH_NO_SETUP="1",
)
subprocess.run(["sh", str(TESTS.parents[1] / "install.sh")], env=env, check=True)
fixture = smoke.Installation(app / "bin", root)
state = root / "state"
manager = ["systemctl"] + ([] if os.getuid() == 0 else ["--user"])
label = (
    "cuttlefish-" + hashlib.sha256(str(state).encode()).hexdigest()[:32] + ".service"
)
command = [str(app / "bin/cuttlefish"), "--state-dir", str(state)]


def service(action):
    subprocess.run(
        command
        + ["service", action, "--listen", fixture.origin.removeprefix("http://")],
        check=True,
        timeout=90,
    )


def pid():
    return int(
        subprocess.check_output(
            manager + ["show", label, "--property=MainPID", "--value"], text=True
        )
    )


def job(phase):
    record = {
        "available": None,
        "checked": True,
        "check_error": None,
        "job": {
            "id": str(uuid.uuid4()),
            "version": tag,
            "phase": phase,
            "restart_only": False,
            "error": None,
        },
    }
    # A saved older marker forces actual extraction and release activation.
    # The executable version remains this build: no fake version validation.
    (app / "current/.release-version").write_text("v0.2.0-alpha.0")
    (state / "updates").mkdir(exist_ok=True)
    (state / "updates/status.json").write_text(json.dumps(record))
    return record["job"]["id"]


def complete(identity):
    status = fixture.api("/api/local/updates")
    assert status["automatic_restart"]
    current = status["job"]
    assert current["id"] == identity
    if current["phase"] == "failed":
        raise AssertionError(current["error"])
    return current["phase"] == "complete"


try:
    fixture.start(setup=True)
    fixture.api(
        "/api/local/setup",
        {
            "token": (state / "setup.token").read_text(),
            "username": "operator",
            "password": "isolated-systemd-test-password",
        },
    )
    original = fixture.api("/api/local/config")["installation_id"]
    fixture.api("/api/local/database", {"mode": "later"})
    fixture.stop()
    subprocess.run(
        command + ["setup", "--listen", fixture.origin.removeprefix("http://")],
        check=True,
        timeout=90,
    )
    first_pid = pid()
    marker = state / "saved-work.txt"
    marker.write_text("existing work")
    prompt = state / "prompts/custom.md"
    prompt.write_text("edited instructions")
    previous = (app / "current").resolve()
    identity = job("failed")
    fixture.api("/api/local/updates", {"action": "retry"})
    smoke.wait_for(lambda: complete(identity))
    assert pid() != first_pid
    assert (app / "current").resolve() != previous
    assert previous.is_dir()
    assert fixture.api("/api/local/config")["installation_id"] == original
    assert marker.read_text() == "existing work"
    assert prompt.read_text() == "edited instructions"
    assert (provider / "auth.json").read_text() == "isolated provider credential marker"
    print(
        "PASS: authenticated dashboard update, real install and automatic restart; saved identity, work, prompt, login and provider marker retained",
        flush=True,
    )

    # A persisted active job makes the CLI resume without another catalog call.
    identity = job("queued")
    previous_pid = pid()
    subprocess.run(command + ["update"], check=True, timeout=180)
    assert complete(identity)
    assert pid() != previous_pid
    print(
        "PASS: cuttlefish update resumes installation and restarts systemd", flush=True
    )

    # Crash at the durable restart boundary; boot recovery owns the same job.
    identity = job("restarting")
    (app / "current/.release-version").write_text(tag)
    previous_pid = pid()
    os.kill(previous_pid, signal.SIGKILL)
    smoke.wait_for(lambda: complete(identity))
    assert pid() != previous_pid
    assert fixture.api("/api/local/config")["installation_id"] == original
    print(
        "PASS: abrupt supervisor restart recovers the same update and authenticated session",
        flush=True,
    )
finally:
    fixture.stop()
    service("uninstall")
    fixture.registry.shutdown()
