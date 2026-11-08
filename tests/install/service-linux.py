#!/usr/bin/env python3
"""Real systemd unit generation with isolated command doubles; no host services."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "install/service-linux.sh"
FAKE = r"""#!/usr/bin/env python3
import json,os,pathlib,sys
p=pathlib.Path(os.environ['SERVICE_FIXTURE']);s=json.loads(p.read_text());name=pathlib.Path(sys.argv[0]).name;args=sys.argv[1:];code=0
s.setdefault('calls',[]).append([name,*args])
if name=='uname': print(s.get('os','Linux'))
elif name=='id': print(s.get('uid',1000))
elif name=='sleep': pass
elif name=='ss':
 if s.get('ss_error'): code=1
 elif s.get('occupied') or s.get('active'): print('LISTEN')
elif name=='curl': code=0 if s.get('active') and not s.get('unhealthy') else 1
elif name=='loginctl':
 if args[0]=='show-user': print('yes' if s.get('linger') else 'no')
 elif s.get('deny_linger'): code=1
 else:s['linger']=True
elif name=='systemctl':
 args=[a for a in args if a!='--user'];op=args[0]
 if op==s.get('fail'): code=1
 elif op=='show-environment': pass
 elif op=='show': print(s.get('fragment',''))
 elif op=='is-active': code=0 if s.get('active') else 3
 elif op=='link':s['fragment']=args[-1]
 elif op=='daemon-reload':pass
 elif op=='enable':s['active']=True;s['enabled']=True;s['fragment']=args[-1]
 elif op=='stop':
  if not s.get('stuck'):s['active']=False
 elif op=='disable':s['active']=False;s['enabled']=False;s['fragment']=''
 else:code=1
p.write_text(json.dumps(s));sys.exit(code)
"""


class ServiceContract(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="cuttlefish-systemd-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / 'home % "$ account'
        self.state = self.home / "state"
        self.state.mkdir(parents=True)
        (self.state / "setup.sqlite").write_text("preserve")
        self.binary = self.home / "bin with spaces/cuttlefish"
        self.binary.parent.mkdir()
        self.binary.write_text("#!/bin/sh\nexit 0\n")
        self.binary.chmod(0o700)
        self.mock = self.root / "mocks"
        self.mock.mkdir()
        for name in ["uname", "id", "sleep", "ss", "curl", "systemctl", "loginctl"]:
            p = self.mock / name
            p.write_text(FAKE)
            p.chmod(0o700)
        self.fixture = self.root / "fixture.json"
        self.fixture.write_text("{}")
        self.env = dict(
            os.environ,
            SERVICE_FIXTURE=str(self.fixture),
            PATH=str(self.mock) + os.pathsep + os.environ["PATH"],
            CODEX_HOME=str(self.home / "codex"),
            UNRELATED_SECRET="not-in-service",
        )

    def saved(self):
        return json.loads(self.fixture.read_text())

    def configure(self, **values):
        s = self.saved()
        s.update(values)
        self.fixture.write_text(json.dumps(s))

    def unit(self):
        return (
            self.state
            / "service"
            / (
                "cuttlefish-"
                + hashlib.sha256(str(self.state).encode()).hexdigest()[:32]
                + ".service"
            )
        )

    def action(self, action, ok=True, listen="127.0.0.1:65432"):
        host, port = listen.rsplit(":", 1)
        r = subprocess.run(
            [
                "sh",
                str(SCRIPT),
                action,
                str(self.state),
                str(self.binary),
                listen,
                host.strip("[]"),
                port,
                str(self.home),
            ],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(r.returncode == 0, ok, r.stdout + r.stderr)
        return r

    def test_lifecycle_and_preservation_for_user_and_root(self):
        for uid in [1000, 0]:
            with self.subTest(uid=uid):
                self.configure(uid=uid)
                self.action("install")
                p = self.unit()
                text = p.read_text()
                self.assertIn("KillMode=process", text)
                self.assertIn("KillSignal=SIGINT", text)
                self.assertIn('home %% \\"$ account', text)
                self.assertNotIn("UNRELATED_SECRET", text)
                self.assertIn(
                    "WantedBy="
                    + ("multi-user.target" if uid == 0 else "default.target"),
                    text,
                )
                self.assertEqual(p.stat().st_mode & 0o777, 0o600)
                self.action("install")
                self.assertEqual(p.read_text(), text)
                linked = self.root / "linked.service"
                linked.unlink(missing_ok=True)
                linked.symlink_to(p)
                self.configure(fragment=str(linked))
                self.action("status")
                for action in [
                    "status",
                    "restart",
                    "stop",
                    "status",
                    "start",
                    "uninstall",
                    "uninstall",
                    "status",
                    "stop",
                ]:
                    self.action(action, listen="127.0.0.1:8080")
                self.assertFalse(p.exists())
                self.assertEqual((self.state / "setup.sqlite").read_text(), "preserve")
                self.assertFalse(self.saved()["active"])
                self.assertFalse(self.saved()["enabled"])
        self.assertTrue(
            all("65432" in c[-1] for c in self.saved()["calls"] if c[0] == "curl")
        )

    def test_preconditions_and_manager_failure(self):
        self.configure(os="Darwin")
        self.action("install", False)
        self.configure(os="Linux")
        self.action("invalid", False)
        self.action("start", False)
        self.configure(fail="show-environment")
        self.action("install", False)
        self.configure(fail="show")
        self.action("install", False)
        self.configure(fail=None)
        self.configure(ss_error=True)
        self.action("install", False)
        self.configure(ss_error=False)
        self.binary.chmod(0o600)
        self.action("install", False)
        self.binary.chmod(0o700)
        (self.state / "setup.sqlite").unlink()
        self.action("install", False)

    def test_port_conflict_does_not_change_another_process(self):
        self.configure(occupied=True)
        self.action("install", False)
        self.assertFalse(self.unit().exists())
        self.assertFalse(
            any(c[0] == "systemctl" and "stop" in c for c in self.saved()["calls"])
        )
        self.configure(occupied=False)
        self.action("install")
        self.action("stop")
        self.configure(occupied=True)
        self.action("start", False)

    def test_foreign_units_links_and_invalid_saved_listener_are_rejected(self):
        self.configure(fragment="/unrelated/service")
        self.action("install", False)
        self.configure(fragment="")
        self.action("install")
        p = self.unit()
        self.action("stop")
        p.write_text("unrelated")
        self.action("install", False)
        self.action("uninstall", False)
        p.unlink()
        p.symlink_to(self.state / "setup.sqlite")
        self.action("install", False)
        p.unlink()
        self.action("install")
        p.write_text(
            p.read_text().replace("# Cuttlefish listener: 127.0.0.1:65432", "# removed")
        )
        self.action("start", False)

    def test_linger_and_lifecycle_failures_are_visible_and_retryable(self):
        self.configure(deny_linger=True)
        self.action("install", False)
        self.configure(deny_linger=False)
        self.action("install")
        for failure in ["disable", "stop", "enable"]:
            self.configure(fail=failure)
            self.action(
                {"disable": "stop", "stop": "restart", "enable": "start"}[failure],
                False,
            )
            self.configure(fail=None)
        self.action("start")
        self.configure(unhealthy=True)
        self.action("status", False)
        self.action("start", False)
        self.configure(unhealthy=False)
        self.configure(stuck=True)
        self.action("restart", False)
        self.configure(stuck=False)
        self.action("uninstall")

    def test_symlinked_directory_and_environment_linebreaks_are_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.state / "service").symlink_to(outside)
        self.action("install", False)
        (self.state / "service").unlink()
        for value in ["bad\npath", "bad\rpath"]:
            self.env["CODEX_HOME"] = value
            self.action("install", False)
            self.assertFalse(self.unit().exists())


if __name__ == "__main__":
    unittest.main()
