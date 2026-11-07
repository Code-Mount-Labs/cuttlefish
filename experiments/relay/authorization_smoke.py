"""Exercise real frp against the Rust issuer, hook and gateway on loopback."""
import argparse
import contextlib
import json
import os
import pathlib
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def request(origin, path, value=None, token=None):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(origin + path, data=None if value is None else json.dumps(value).encode(), headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=3) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()


def await_check(check, label, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise AssertionError(label)


def run(args):
    repo = pathlib.Path(__file__).resolve().parents[2]
    bins = pathlib.Path(args.binaries).resolve()
    frp = pathlib.Path(args.frp).resolve()
    schema = 'relay_smoke_' + uuid.uuid4().hex
    database = args.database
    def sql(statement):
        subprocess.run(['psql', database, '-v', 'ON_ERROR_STOP=1', '-q', '-c', statement], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sql('CREATE SCHEMA ' + schema)
    separator = '&' if '?' in database else '?'
    url = database + separator + urllib.parse.urlencode({'options': '-csearch_path=' + schema})
    children = []
    logs = []
    with tempfile.TemporaryDirectory(prefix='cuttlefish-relay-auth-') as tmp:
        root = pathlib.Path(tmp)
        root.chmod(0o700)
        try:
            issuer_port, gateway_port, hook_port, frp_port, proxy_port = [port() for _ in range(5)]
            issuer = f'http://127.0.0.1:{issuer_port}'
            gateway = f'http://127.0.0.1:{gateway_port}'
            private = (repo / 'crates/platform/tests/fixtures/public-test-key.pem').read_text()
            public = (repo / 'crates/platform/tests/fixtures/public-test-key.txt').read_text()
            env = dict(os.environ, DATABASE_URL=url, PUBLIC_ORIGIN=issuer, RELAY_ORIGIN=gateway, RELAY_PUBLIC_KEY=public)
            def start(name, argv, extra=None):
                log = (root / (name + '.log')).open('ab')
                logs.append(log)
                child = subprocess.Popen(argv, env=dict(env, **(extra or {})), stdout=log, stderr=log)
                children.append(child)
                return child
            platform = start('platform', [str(bins / 'cuttlefish-platform'), '--port', str(issuer_port)], {'RELAY_PRIVATE_KEY': private})
            await_check(lambda: request(issuer, '/healthz')[0] == 200, 'issuer startup')
            relay_args = [str(bins / 'cuttlefish-relay'), '--listen', f'127.0.0.1:{gateway_port}', '--hook', f'127.0.0.1:{hook_port}', '--upstream', f'http://127.0.0.1:{proxy_port}']
            relay = start('gateway', relay_args)
            await_check(lambda: request(gateway, '/healthz')[0] == 200, 'gateway startup')
            account = request(issuer, '/api/v1/auth/register', {'username': 'relay-' + uuid.uuid4().hex[:12], 'password': secrets.token_hex(24)})[1]['token']
            group = request(issuer, '/api/v1/groups', {'name': 'Relay transport test'}, account)[1]['group_id']
            invite = request(issuer, f'/api/v1/groups/{group}/invitations', {}, account)[1]['invitation']
            identity = str(uuid.uuid4())
            credential, connector = secrets.token_hex(32), secrets.token_hex(32)
            registration = {'group_id': group, 'installation_id': identity, 'role': 'router', 'name': 'Loopback test', 'router_url': issuer, 'credential': credential, 'invitation': invite}
            assert request(issuer, '/api/v1/installations', registration)[0] == 200
            lease = request(issuer, '/api/v1/installations/relay', {'revision': 1, 'credential': connector}, credential)[1]
            config = {'bindAddr': '127.0.0.1', 'bindPort': frp_port, 'proxyBindAddr': '127.0.0.1', 'vhostHTTPPort': proxy_port, 'log': {'level': 'error'}, 'auth': {'method': 'oidc', 'additionalScopes': ['HeartBeats', 'NewWorkConns'], 'oidc': {'issuer': issuer, 'audience': gateway}}, 'httpPlugins': [{'name': 'authorization', 'addr': f'127.0.0.1:{hook_port}', 'path': '/frp', 'ops': ['Login', 'NewProxy', 'Ping', 'NewWorkConn']}]}
            (root / 'frps.json').write_text(json.dumps(config))
            transport_args = [str(frp / 'frps'), '-c', str(root / 'frps.json')]
            transport = start('frps', transport_args)
            config = {'serverAddr': '127.0.0.1', 'serverPort': frp_port, 'user': identity, 'loginFailExit': False, 'log': {'level': 'error'}, 'transport': {'heartbeatInterval': 1, 'heartbeatTimeout': 5}, 'auth': {'method': 'oidc', 'additionalScopes': ['HeartBeats', 'NewWorkConns'], 'oidc': {'clientID': identity, 'clientSecret': connector, 'tokenEndpointURL': issuer + '/api/v1/relay/token'}}, 'proxies': [{'name': lease['epoch'] + '.dashboard', 'type': 'http', 'localIP': '127.0.0.1', 'localPort': issuer_port, 'customDomains': [identity + '.' + lease['epoch'] + '.cuttlefish.invalid'], 'locations': ['/']}]}
            (root / 'frpc.json').write_text(json.dumps(config))
            client_args = [str(frp / 'frpc'), '-c', str(root / 'frpc.json')]
            client = start('frpc', client_args)
            path = '/i/' + identity + '/v1/probe'
            # The issuer has no /v1/probe. A routed JSON 404 is deliberately not enough:
            # serve an allowed JSON route via a dedicated fixture instead.
            import http.server
            import threading
            class Origin(http.server.BaseHTTPRequestHandler):
                def log_message(self, *unused): pass
                def do_GET(self):
                    body = json.dumps({'identity': identity, 'path': self.path}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
            origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
            threading.Thread(target=origin.serve_forever, daemon=True).start()
            client.terminate(); client.wait(timeout=5)
            config['proxies'][0]['localPort'] = origin.server_port
            (root / 'frpc.json').write_text(json.dumps(config))
            client = start('frpc', client_args)
            def routed():
                status, body = request(gateway, path)
                return status == 200 and body.get('identity') == identity
            await_check(routed, 'authenticated frp route')
            for child, name, argv in [(client, 'frpc', client_args), (transport, 'frps', transport_args), (relay, 'gateway', relay_args)]:
                child.send_signal(signal.SIGINT); child.wait(timeout=5)
                start(name, argv)
                await_check(routed, name + ' restart recovery')
            assert request(issuer, '/api/v1/installations/relay', {'revision': 2, 'credential': None}, credential)[0] == 200
            assert request(gateway, path)[0] == 401
            origin.shutdown(); origin.server_close()
            print(json.dumps({'status': 'passed', 'checks': ['real frp OIDC client credentials and private authorization hook', 'installation-specific HTTP route', 'connector restart', 'transport restart', 'Rust gateway restart', 'immediate durable revocation']}, indent=2))
        except Exception:
            # Logs stay private; only protocol errors needed for diagnosis are printed.
            for path in root.glob('*.log'):
                text = path.read_text(errors='replace')
                for secret in [locals().get('connector', ''), locals().get('credential', ''), locals().get('account', '')]:
                    if secret: text = text.replace(secret, '[redacted]')
                print(path.name, text[-3000:])
            raise
        finally:
            for child in reversed(children):
                if child.poll() is None: child.send_signal(signal.SIGINT)
            for child in children:
                try: child.wait(timeout=5)
                except subprocess.TimeoutExpired: child.kill(); child.wait()
            for log in logs: log.close()
            sql('DROP SCHEMA ' + schema + ' CASCADE')

if __name__ == '__main__':
    if not __debug__: raise RuntimeError('Assertions must be enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binaries', default='target/debug')
    parser.add_argument('--frp', required=True)
    parser.add_argument('--database', required=True)
    run(parser.parse_args())
