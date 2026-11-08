#!/usr/bin/env python3
"""Run only in a disposable Linux VM: real systemd lifecycle and reboot recovery.

Usage: service-linux-live.py <adapter> <private-test-directory> [--leave-for-reboot|--after-reboot]
Run as a regular user or root. By default it cleans up its test service.
For reboot tests, use --leave-for-reboot, reboot the VM, then --after-reboot.
"""

import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import urllib.request

script = Path(sys.argv[1]).resolve()
root = Path(sys.argv[2]).resolve()
if "--after-reboot" not in sys.argv:
    root.mkdir(parents=True, exist_ok=False)
state = root / 'state % $ " spaces \\ trailing  '
home = Path.home()
proof = root / "proof.json"
manager = ["systemctl"] + ([] if os.getuid() == 0 else ["--user"])


def wait(read, label):
    end = time.monotonic() + 45
    while time.monotonic() < end:
        try:
            value = read()
            if value:
                return value
        except (OSError, ValueError):
            pass
        time.sleep(0.2)
    raise RuntimeError("Timed out: " + label)


def api():
    with urllib.request.urlopen(
        "http://127.0.0.1:" + str(port) + "/api/local/status", timeout=2
    ) as response:
        return json.load(response)


def action(name):
    subprocess.run(
        [
            "sh",
            str(script),
            name,
            str(state),
            str(binary),
            "127.0.0.1:" + str(port),
            "127.0.0.1",
            str(port),
            str(home),
        ],
        check=True,
        timeout=90,
    )


def alive(pid):
    os.kill(pid, 0)
    return True


if "--after-reboot" in sys.argv:
    data = json.loads(proof.read_text())
    port = data["port"]
    binary = Path(data["binary"])
    after = wait(api, "automatic startup after reboot")
    assert after["boot"] != data["boot"]
    assert after["retained"] == "saved work"
    action("uninstall")
    (state / "worker-stop").touch()
    print("PASS: reboot autostart and retained data; isolated service removed")
    sys.exit(0)

state.mkdir(exist_ok=True)
(state / "setup.sqlite").write_text("fixture")
(state / "retained.txt").write_text("saved work")
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
binary = root / 'fixture % $ " executable'
binary.write_text(
    "#!"
    + sys.executable
    + "\n"
    + r"""import http.server,json,os,subprocess,sys,time,uuid
from pathlib import Path
state=Path(sys.argv[sys.argv.index('--state-dir')+1]);port=int(sys.argv[-1].rsplit(':',1)[1]);boot=Path('/proc/sys/kernel/random/boot_id').read_text().strip()
if '--worker' in sys.argv:
 while not (state/'worker-stop').exists():
  (state/'heartbeat').write_text(str(time.time()));time.sleep(.1)
 sys.exit(0)
worker=None
try:
 old=json.loads((state/'worker.json').read_text())
 if old['boot']==boot:os.kill(old['pid'],0);worker=old['pid']
except (OSError,ValueError):pass
if worker is None:
 child=subprocess.Popen([sys.executable,__file__,'--worker','--state-dir',str(state),'--listen','127.0.0.1:'+str(port)]);worker=child.pid
 (state/'worker.json').write_text(json.dumps({'boot':boot,'pid':worker}))
instance=str(uuid.uuid4())
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  data=json.dumps({'pid':os.getpid(),'worker':worker,'instance':instance,'home':os.environ['HOME'],'cwd':os.getcwd(),'retained':(state/'retained.txt').read_text(),'boot':boot}).encode()
  self.send_response(200);self.end_headers();self.wfile.write(data)
 def log_message(self,*args):pass
http.server.HTTPServer(('127.0.0.1',port),Handler).serve_forever()
"""
)
binary.chmod(0o700)
label = (
    "cuttlefish-" + hashlib.sha256(str(state).encode()).hexdigest()[:32] + ".service"
)
try:
    action("install")
    first = api()
    assert first["home"] == str(home)
    assert first["cwd"] == str(state)
    subprocess.run(
        ["systemd-analyze", "verify", str(state / "service" / label)], check=True
    )
    action("install")
    assert api()["instance"] == first["instance"]
    os.kill(first["pid"], signal.SIGKILL)
    second = wait(
        lambda: (v if (v := api())["instance"] != first["instance"] else None),
        "crash restart",
    )
    assert second["worker"] == first["worker"]
    assert alive(second["worker"])
    action("restart")
    third = api()
    assert third["instance"] != second["instance"]
    assert third["worker"] == first["worker"]
    # Refusing a conflicting listener must never kill that listener.
    action("stop")
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))
        listener.listen()
        result = subprocess.run(
            [
                "sh",
                str(script),
                "start",
                str(state),
                str(binary),
                "127.0.0.1:" + str(port),
                "127.0.0.1",
                str(port),
                str(home),
            ],
            capture_output=True,
        )
        assert result.returncode != 0
    action("start")
    action("status")
    assert api()["worker"] == first["worker"]
    proof.write_text(
        json.dumps({"port": port, "binary": str(binary), "boot": first["boot"]})
    )
    print(
        "PASS: systemd lifecycle, idempotency, crash restart, retained specialist, quoting, and busy-port protection"
    )
    if "--leave-for-reboot" not in sys.argv:
        action("uninstall")
        (state / "worker-stop").touch()
except BaseException:
    subprocess.run(manager + ["status", label, "--no-pager"], check=False)
    subprocess.run(
        ["journalctl"]
        + ([] if os.getuid() == 0 else ["--user"])
        + ["-u", label, "-n", "40", "--no-pager"],
        check=False,
    )
    (state / "worker-stop").touch()
    action("uninstall")
    raise
