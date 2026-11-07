"""Use a disposable Mac installation to verify an explicitly selected staging relay."""
import argparse
import fcntl
import http.client
import json
import os
import pathlib
import pty
import re
import secrets
import shlex
import signal
import sqlite3
import subprocess
import tempfile
import termios
import time
import urllib.error
import urllib.request
import uuid
from authorization_smoke import port, await_check


def call(origin, path, body=None, bearer=None, cookie=None, browser=False):
    headers = {'Content-Type': 'application/json'}
    if bearer: headers['Authorization'] = 'Bearer ' + bearer
    if cookie: headers['Cookie'] = cookie
    if browser:
        from urllib.parse import urlsplit
        url = urlsplit(origin)
        headers['Origin'] = url.scheme + '://' + url.netloc
    request = urllib.request.Request(origin + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return response.status, json.load(response), response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode(), error.headers


def run(args):
    if os.uname().sysname != 'Darwin': raise RuntimeError('This test requires macOS launchd')
    binary = pathlib.Path(args.binary).resolve()
    username = 'smoke-' + uuid.uuid4().hex[:15]
    password = secrets.token_hex(24)
    account = call(args.platform, '/api/v1/auth/register', {'username': username, 'password': password})[1]['token']
    group = call(args.platform, '/api/v1/groups', {'name': 'Disposable Mac relay test ' + username}, account)[1]['group_id']
    invitation = call(args.platform, f'/api/v1/groups/{group}/invitations', {}, account)[1]['invitation']
    children = []
    with tempfile.TemporaryDirectory(prefix='cuttlefish-mac-relay-') as temporary:
        root = pathlib.Path(temporary)
        state = root / 'state'
        local = 'http://127.0.0.1:' + str(port())
        address = local.removeprefix('http://')
        log = (root / 'runtime.log').open('wb')
        terminals = []
        if args.installer_url:
            binary = root / 'installed/bin/cuttlefish'
        environment = dict(os.environ, CUTTLEFISH_INSTALL_DIR=str(root / 'installed'))
        environment.pop('CUTTLEFISH_NO_SETUP', None)
        environment.pop('CUTTLEFISH_VERSION', None)
        download = shlex.join(['curl', '--proto', '=https', '--tlsv1.2', '-fsSL', args.installer_url or ''])
        def start(mode):
            command = [str(binary), '--state-dir', str(state), mode, '--listen', address]
            if mode == 'setup': command += ['--platform', args.platform]
            installer = mode == 'setup' and args.installer_url
            if installer:
                master, slave = pty.openpty()
                terminals.append(master)
                def control_terminal():
                    os.setsid()
                    fcntl.ioctl(0, termios.TIOCSCTTY, 0)
                invocation = download + ' | ' + shlex.join(['sh', '-s', '--', *command[1:3], *command[4:]])
                try:
                    process = subprocess.Popen(['/bin/sh', '-c', invocation], stdin=slave, stdout=log, stderr=log, env=environment, preexec_fn=control_terminal)
                finally:
                    os.close(slave)
            else:
                process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=log, stderr=log, start_new_session=True)
            children.append(process)
            if mode == 'setup':
                # Fresh groups offer combined, then router-only. Select router-only;
                # database provisioning is independent of the relay being tested.
                answers = f'{group}\n{invitation}\n2\nDisposable Mac relay\nmanaged\n'.encode()
                if installer:
                    os.write(master, answers)
                else:
                    process.stdin.write(answers)
            if process.stdin:
                process.stdin.close()
            return process
        def status():
            return call(local, '/api/local/remote', cookie=local_cookie)[1]
        def service_pid():
            config = next((state / 'relay').glob('*.json'))
            label = config.stem
            output = subprocess.check_output(['launchctl', 'print', f'gui/{os.getuid()}/{label}'], text=True)
            return int(re.search(r'\bpid = (\d+)', output).group(1))
        def descendants(process):
            rows = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,command='], text=True)
            processes = [line.strip().split(None, 2) for line in rows.splitlines()]
            owned = {process.pid}
            for _ in processes:
                more = {int(pid) for pid, parent, _ in processes if int(parent) in owned}
                if more <= owned: break
                owned |= more
            return [(int(pid), command) for pid, _, command in processes if int(pid) in owned]
        def stop(process):
            # The pipeline shell is not the supervisor. Signal the exact runtime
            # descendant, then require the installer pipeline to finish cleanly.
            runtimes = [pid for pid, command in descendants(process)
                        if command.startswith(str(binary) + ' --state-dir ' + str(state) + ' ')]
            assert len(runtimes) == 1, 'exactly one owned supervisor must be running'
            os.kill(runtimes[0], signal.SIGINT)
            assert process.wait(timeout=30) == 0
        local_cookie = None
        try:
            process = start('setup')
            await_check(lambda: call(local, '/api/local/status')[0] == 200, 'Mac setup listener', 180)
            if args.expected_version:
                assert subprocess.check_output([str(binary), '--version'], text=True).strip() == 'cuttlefish-machine ' + args.expected_version
            setup = call(local, '/api/local/setup', {'token': (state / 'setup.token').read_text(), 'username': 'operator', 'password': password}, browser=True)
            assert setup[0] == 200, 'local administrator setup'
            local_cookie = setup[2]['Set-Cookie'].split(';')[0]
            await_check(lambda: status().get('state') == 'ready', 'managed installation readiness', 180)
            remote = status()['public_url']
            assert remote.startswith(args.relay + '/i/')
            first = service_pid()
            assert call(remote, '/api/local/status')[0] == 200
            assert call(remote, '/api/local/config')[0] == 401
            login = call(remote, '/api/local/login', {'username': 'operator', 'password': password}, browser=True)
            assert login[0] == 200, 'public dashboard login'
            assert 'Secure' in login[2]['Set-Cookie'] and ('Path=' + remote.removeprefix(args.relay) + '/') in login[2]['Set-Cookie']
            remote_cookie = login[2]['Set-Cookie'].split(';')[0]
            config = call(remote, '/api/local/config', cookie=remote_cookie)
            assert config[0] == 200 and config[1]['group_id'] == group
            assert call(remote, '/api/local/setup', {}, browser=True)[0] == 400
            stop(process)
            assert service_pid() == first, 'connector must survive supervisor exit'
            if args.installer_url:
                # Reinstall public release bytes without altering the durable identity.
                subprocess.run(['/bin/sh', '-c', download + ' | sh'], env=dict(environment, CUTTLEFISH_NO_SETUP='1'), check=True, stdout=log, stderr=log, timeout=180)
            process = start('serve')
            await_check(lambda: call(remote, '/api/local/status')[0] == 200, 'supervisor restart')
            assert service_pid() == first, 'supervisor must adopt the existing connector'
            checks = ['Mac setup selects managed access', 'pinned native connector installation', 'public HTTPS reaches local JSON API', 'dashboard login and path-scoped secure cookie', 'remote bootstrap blocked', 'connector survives supervisor restart without replacement']
            if args.installer_url:
                checks += ['public curl-to-shell installation with interactive managed setup', 'repeat installation preserves identity and native connector']
            if args.restart_service:
                command = ['railway', 'restart', '--project', args.project, '--environment', args.environment, '--service', args.restart_service, '--yes', '--json']
                environment = dict(os.environ, RAILWAY_CALLER='skill:use-railway@1.7.0', RAILWAY_AGENT_SESSION='cuttlefish-relay-20261107')
                subprocess.run(command, check=True, env=environment, stdout=subprocess.DEVNULL)
                time.sleep(8)
                await_check(lambda: call(remote, '/api/local/status')[0] == 200, 'Railway relay restart recovery', 90)
                assert service_pid() == first
                checks.append('Railway relay restart reconnects the same native connector')
            assert call(local, '/api/local/remote', {'mode': 'local'}, cookie=local_cookie, browser=True)[0] == 202
            await_check(lambda: status().get('state') == 'local', 'local-only withdrawal', 30)
            assert call(remote, '/api/local/status')[0] == 401
            assert call(local, '/api/local/status')[0] == 200
            stop(process)
            process = start('serve')
            await_check(lambda: call(local, '/api/local/status')[0] == 200, 'local-only restart')
            assert call(remote, '/api/local/status')[0] == 401
            checks += ['local-only revokes public route and preserves local access', 'withdrawal survives another supervisor restart']
            print(json.dumps({'status': 'passed', 'platform': args.platform, 'relay': args.relay, 'checks': checks}, indent=2))
        except Exception:
            if local_cookie:
                try: print(json.dumps({'last_remote_status': status()}))
                except Exception: pass
            raise
        finally:
            for process in children:
                if process.poll() is None:
                    owned = descendants(process)
                    for pid, _ in reversed(owned):
                        try: os.kill(pid, signal.SIGTERM)
                        except (ProcessLookupError, PermissionError): pass
                    try: process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        for pid, _ in reversed(owned):
                            try: os.kill(pid, signal.SIGKILL)
                            except (ProcessLookupError, PermissionError): pass
                        process.wait(timeout=15)
            # Stop the supervisor first so it cannot recreate a connector during cleanup.
            for manifest in (state / 'relay').glob('*.plist'):
                subprocess.run(['launchctl', 'bootout', f'gui/{os.getuid()}/{manifest.stem}'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if (state / 'setup.sqlite').exists():
                with sqlite3.connect(state / 'setup.sqlite') as journal:
                    row = journal.execute('SELECT registration FROM installation').fetchone()
                    remote_row = journal.execute('SELECT revision FROM remote_access').fetchone()
                    revision = remote_row[0] if remote_row else 0
                if row:
                    registration = json.loads(row[0])
                    call(args.platform, '/api/v1/installations/relay', {'revision': revision + 1, 'credential': None}, registration['credential'])
            for terminal in terminals: os.close(terminal)
            log.close()

if __name__ == '__main__':
    if not __debug__: raise RuntimeError('Assertions must be enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--installer-url', help='Public HTTPS installer to run through curl | sh with a controlling terminal')
    parser.add_argument('--expected-version', help='Assert the installed binary version')
    parser.add_argument('--binary', default='target/debug/cuttlefish')
    parser.add_argument('--platform', required=True)
    parser.add_argument('--relay', required=True)
    parser.add_argument('--project')
    parser.add_argument('--environment')
    parser.add_argument('--restart-service')
    run(parser.parse_args())
