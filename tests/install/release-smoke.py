#!/usr/bin/env python3
"""Exercise shipped binaries against a local registry fixture and real Docker.

The fixture only replaces the hosted directory. Authentication, the local
journal, PostgreSQL provisioning, agent enrollment and restarts are real.
Pass the installed bin directory and an empty directory for retained test logs.
"""
import contextlib
import http.cookiejar
import http.server
import json
import pathlib
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid


def unused_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(check, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (OSError, urllib.error.URLError, json.JSONDecodeError):
            pass
        time.sleep(0.2)
    raise AssertionError("Timed out waiting for installation readiness")


class Installation:
    def __init__(self, binaries, root):
        self.binaries, self.root = binaries, root
        self.origin = f"http://127.0.0.1:{unused_port()}"
        self.group = str(uuid.uuid4())
        self.child = None
        self.log = (root / "runtime.log").open("w")
        self.client = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
        )
        self.heartbeats = []
        owner = self

        class Registry(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if self.path == "/api/v1/installations/preflight":
                    response = {"roles": ["router_agent"], "router_url": None}
                elif self.path == "/api/v1/installations":
                    response = {
                        "group_id": data["group_id"],
                        "installation_id": data["installation_id"],
                        "router_url": owner.origin,
                    }
                elif self.path == "/api/v1/router/authorize-agent":
                    response = {"group_id": owner.group}
                elif self.path == "/api/v1/installations/heartbeat":
                    owner.heartbeats.append(data)
                    response = {"ok": True}
                else:
                    self.send_error(404)
                    return
                body = json.dumps(response).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.registry = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Registry)
        threading.Thread(target=self.registry.serve_forever, daemon=True).start()

    def api(self, path, body=None):
        request = urllib.request.Request(
            self.origin + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json", "Origin": self.origin},
        )
        with self.client.open(request, timeout=5) as response:
            return json.load(response)

    def start(self, setup=False):
        args = [str(self.binaries / "cuttlefish"), "--state-dir", str(self.root / "state")]
        args += ["setup" if setup else "serve", "--listen", self.origin.removeprefix("http://")]
        if setup:
            args += ["--platform", f"http://127.0.0.1:{self.registry.server_port}"]
        self.child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=self.log,
                                      stderr=self.log, text=True)
        if setup:
            self.child.stdin.write(f"{self.group}\n{'b' * 64}\n1\nFresh OS test\n{self.origin}\n")
        self.child.stdin.close()
        wait_for(lambda: self.api("/api/local/status"), 30)

    def stop(self):
        if self.child and self.child.poll() is None:
            self.child.send_signal(signal.SIGINT)
            try:
                self.child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
                raise

    def spec(self):
        with contextlib.closing(sqlite3.connect(self.root / "state/setup.sqlite")) as db:
            encoded = db.execute("SELECT spec FROM resources").fetchone()[0]
            return json.loads(encoded) if encoded else None

    def sql(self, text):
        result = subprocess.run(
            ["docker", "exec", self.spec()["container"], "psql", "-U", "postgres",
             "-d", "cuttlefish", "-At", "-v", "ON_ERROR_STOP=1", "-c", text],
            check=True, text=True, capture_output=True,
        )
        return result.stdout.strip()

    def exercise(self):
        self.start(setup=True)
        self.api("/api/local/setup", {
            "token": (self.root / "state/setup.token").read_text(),
            "username": "operator", "password": "fresh-os-test-password",
        })
        original = self.api("/api/local/config")["installation_id"]
        self.api("/api/local/database", {"mode": "later"})
        self.stop()
        self.start()
        assert self.api("/api/local/config")["database"]["state"] == "unconfigured"
        self.api("/api/local/database", {"mode": "managed", "port": unused_port()})
        wait_for(lambda: self.api("/api/local/config")["database"]["state"] == "ready", 300)
        wait_for(lambda: self.sql("SELECT count(*) FROM nodes") == "1")
        self.sql("CREATE TABLE installation_marker(value TEXT); INSERT INTO installation_marker VALUES ('survived')")
        before = self.spec()
        self.stop()
        self.start()
        wait_for(lambda: self.api("/api/local/config")["database"]["state"] == "ready")
        assert self.api("/api/local/config")["installation_id"] == original
        assert self.spec() == before
        assert self.sql("SELECT value FROM installation_marker") == "survived"
        assert self.sql("SELECT rolsuper FROM pg_roles WHERE rolname='cuttlefish'") == "f"
        wait_for(lambda: any(value["ready"] for value in self.heartbeats), 40)
        assert all("postgresql://" not in json.dumps(value) for value in self.heartbeats)
        self.api("/api/local/logout", {})
        self.api("/api/local/login", {"username": "operator", "password": "fresh-os-test-password"})
        print("PASS: packaged router+agent, authentication, deferred setup, managed PostgreSQL, enrollment, supervisor restart, durable identity/data", flush=True)

    def close(self):
        self.stop()
        self.registry.shutdown()
        self.registry.server_close()
        spec = self.spec() if (self.root / "state/setup.sqlite").exists() else None
        if spec:
            for args in [["rm", "--force", spec["container"]], ["volume", "rm", spec["volume"]]]:
                subprocess.run(["docker", *args], check=True, capture_output=True)
        self.log.close()


if __name__ == "__main__":
    binaries, root = map(lambda value: pathlib.Path(value).resolve(), sys.argv[1:])
    root.mkdir(parents=True, exist_ok=False)
    installation = Installation(binaries, root)
    try:
        installation.exercise()
    finally:
        installation.close()
