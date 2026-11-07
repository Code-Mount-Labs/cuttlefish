"""Disposable loopback-only frp transport evaluation; not a multi-tenant service."""
import argparse
import base64
import contextlib
import hashlib
import http.client
import http.server
import json
import pathlib
import secrets
import socket
import socketserver
import ssl
import subprocess
import tempfile
import threading
import time

class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

class Bridge(socketserver.BaseRequestHandler):

    def handle(self):
        try:
            with contextlib.ExitStack() as stack:
                incoming = stack.enter_context(self.server.tls.wrap_socket(self.request, server_side=True))
                outgoing = stack.enter_context(socket.create_connection(('127.0.0.1', self.server.target), timeout=5))
                outgoing.settimeout(None)

                def copy(source, target):
                    try:
                        while True:
                            data = source.recv(65536)
                            if not data:
                                break
                            target.sendall(data)
                    except (OSError, ssl.SSLError):
                        pass
                    finally:
                        with contextlib.suppress(OSError):
                            target.shutdown(socket.SHUT_RDWR)
                other = threading.Thread(target=copy, args=(incoming, outgoing), daemon=True)
                other.start()
                copy(outgoing, incoming)
                other.join(timeout=2)
                incoming.close()
                outgoing.close()
        except (OSError, ssl.SSLError):
            pass

class Origin(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path.endswith('/events'):
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(b'data: ready\n\n')
            self.wfile.flush()
            self.server.stream_seen.set()
            self.server.stream_release.wait(10)
            self.close_connection = True
        elif self.path.endswith('/socket'):
            digest = base64.b64encode(hashlib.sha1((self.headers['Sec-WebSocket-Key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
            self.send_response(101)
            self.send_header('Upgrade', 'websocket')
            self.send_header('Connection', 'Upgrade')
            self.send_header('Sec-WebSocket-Accept', digest)
            self.end_headers()
            self.wfile.write(b'\x81\x02ok')
            self.wfile.flush()
            self.close_connection = True
        else:
            body = json.dumps({'path': self.path, 'origin': self.server.label}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def run(binary_directory):
    binaries = pathlib.Path(binary_directory).resolve()
    for name in ['frpc', 'frps']:
        version = subprocess.check_output([str(binaries / name), '--version'], text=True).strip()
        if version != '0.72.0':
            raise ValueError('This experiment is pinned to frp 0.72.0')
    children = []
    servers = []
    files = []
    checks = []
    with tempfile.TemporaryDirectory(prefix='cuttlefish-frp-proof-') as temp:
        root = pathlib.Path(temp)
        try:
            subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(root / 'key.pem'), '-out', str(root / 'cert.pem'), '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost'], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            relay_port = free_port()
            secret = secrets.token_hex(32)
            (root / 'frps.toml').write_text(f'bindAddr = "127.0.0.1"\nbindPort = {relay_port}\nvhostHTTPPort = {relay_port}\nauth.token = "{secret}"\ntransport.heartbeatTimeout = 10\nlog.level = "warn"\n')

            def start(binary, config):
                log = (root / (config + '.log')).open('ab')
                files.append(log)
                p = subprocess.Popen([str(binaries / binary), '-c', str(root / config)], stdout=log, stderr=log)
                children.append(p)
                return p
            relay = start('frps', 'frps.toml')
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.load_cert_chain(root / 'cert.pem', root / 'key.pem')
            edge = Server(('127.0.0.1', 0), Bridge)
            edge.tls = tls
            edge.target = relay_port
            servers.append(edge)
            threading.Thread(target=edge.serve_forever, daemon=True).start()
            edge_port = edge.server_address[1]
            context = ssl.create_default_context(cafile=str(root / 'cert.pem'))

            def request(path):
                c = http.client.HTTPSConnection('localhost', edge_port, context=context, timeout=3)
                c.request('GET', path)
                r = c.getresponse()
                status = r.status
                data = r.read()
                c.close()
                return (status, data)

            def await_path(path, label):
                deadline = time.monotonic() + 35
                while time.monotonic() < deadline:
                    try:
                        (status, data) = request(path)
                        if status == 200 and json.loads(data)['origin'] == label:
                            return
                    except (OSError, http.client.HTTPException, ValueError):
                        pass
                    time.sleep(0.2)
                raise AssertionError('route did not recover: ' + path)

            def client(name, token, verify=True):
                origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
                origin.label = name
                origin.stream_seen = threading.Event()
                origin.stream_release = threading.Event()
                servers.append(origin)
                threading.Thread(target=origin.serve_forever, daemon=True).start()
                trust = f'transport.tls.trustedCaFile = "{root}/cert.pem"\n' if verify else ''
                (root / (name + '.toml')).write_text(f'serverAddr = "localhost"\nserverPort = {edge_port}\nauth.token = "{token}"\nloginFailExit = false\ntransport.protocol = "wss"\ntransport.tls.useSystemCa = true\n{trust}transport.heartbeatInterval = 2\ntransport.heartbeatTimeout = 6\nlog.level = "warn"\n[[proxies]]\nname = "{name}"\ntype = "http"\nlocalIP = "127.0.0.1"\nlocalPort = {origin.server_port}\ncustomDomains = ["localhost"]\nlocations = ["/i/{name}/"]\n')
                return (start('frpc', name + '.toml'), origin)
            (a, origin_a) = client('a', secret)
            (b, _) = client('b', secret)
            await_path('/i/a/hello', 'a')
            await_path('/i/b/hello', 'b')
            checks.append('two installations routed over WSS on one HTTPS listener')
            (status, data) = request('/i/a/hello?x=1')
            assert status == 200 and json.loads(data)['path'] == '/i/a/hello?x=1'
            checks.append('path and query preserved')
            assert request('/i/unknown/')[0] == 404
            checks.append('unknown installation is not forwarded')
            c = http.client.HTTPSConnection('localhost', edge_port, context=context, timeout=3)
            c.request('GET', '/i/a/events')
            r = c.getresponse()
            assert r.status == 200 and r.readline() == b'data: ready\n' and (not origin_a.stream_release.is_set())
            origin_a.stream_release.set()
            c.close()
            checks.append('SSE arrives before upstream completes')
            c = http.client.HTTPSConnection('localhost', edge_port, context=context, timeout=3)
            c.request('GET', '/i/a/socket', headers={'Connection': 'Upgrade', 'Upgrade': 'websocket', 'Sec-WebSocket-Version': '13', 'Sec-WebSocket-Key': base64.b64encode(secrets.token_bytes(16)).decode()})
            r = c.getresponse()
            assert r.status == 101
            assert r.fp.read(4) == b'\x81\x02ok'
            c.close()
            checks.append('WebSocket upgrade and frame delivery')
            a.terminate()
            a.wait(timeout=5)
            a = start('frpc', 'a.toml')
            await_path('/i/a/restarted', 'a')
            checks.append('connector restart recovers same route')
            relay.terminate()
            relay.wait(timeout=5)
            relay = start('frps', 'frps.toml')
            await_path('/i/a/relay-restarted', 'a')
            await_path('/i/b/relay-restarted', 'b')
            checks.append('relay restart reconnects both existing connectors')
            (bad, _) = client('wrong-token', secrets.token_hex(32))
            (untrusted, _) = client('untrusted', secret, verify=False)
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                wrong = (root / 'wrong-token.toml.log').read_text()
                tlslog = (root / 'untrusted.toml.log').read_text()
                if 'token' in wrong and ('certificate' in tlslog or 'x509' in tlslog):
                    break
                time.sleep(0.1)
            else:
                raise AssertionError('negative auth/TLS checks did not run')
            assert request('/i/wrong-token/')[0] == 404 and request('/i/untrusted/')[0] == 404
            checks.extend(['wrong connector token rejected', 'untrusted TLS certificate rejected'])
            print(json.dumps({'transport': 'frp v0.72.0', 'environment': 'loopback TLS edge fixture', 'checks': checks, 'status': 'passed'}, indent=2))
        finally:
            for p in reversed(children):
                if p.poll() is None:
                    p.terminate()
            for p in children:
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()
            for server in servers:
                if hasattr(server, 'stream_release'):
                    server.stream_release.set()
                server.shutdown()
                server.server_close()
            for f in files:
                f.close()
if __name__ == '__main__':
    if not __debug__:
        raise RuntimeError('Run without Python optimization so assertions remain enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-directory', required=True, help='Directory containing the verified frp 0.72.0 frpc and frps binaries')
    run(parser.parse_args().binary_directory)
