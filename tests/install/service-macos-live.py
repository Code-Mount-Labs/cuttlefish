#!/usr/bin/env python3
"""Real launchd lifecycle test with a disposable home, listener, and process.

Does not install into the account's LaunchAgents folder or alter real apps.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

SCRIPT = Path(__file__).resolve().parents[2] / 'install/service-macos.sh'
assert platform.system() == 'Darwin', 'This test requires a logged-in macOS GUI session'

def wait_for(read, label):
    end = time.monotonic() + 45
    while time.monotonic() < end:
        try:
            value = read()
            if value: return value
        except (OSError, ValueError): pass
        time.sleep(.2)
    raise RuntimeError('Timed out: '+label)

with tempfile.TemporaryDirectory(prefix='cuttlefish-launchd-') as folder:
    root=Path(folder).resolve();home=root/'isolated home';state=home/'state';state.mkdir(parents=True)
    (state/'setup.sqlite').write_text('isolated service fixture')
    marker=state/'retained.txt';marker.write_text('retain across restarts')
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
    binary=root/'fixture.py'
    binary.write_text('#!'+sys.executable+'\n'+'''import http.server,json,os,sys
from pathlib import Path
state=Path(sys.argv[sys.argv.index('--state-dir')+1]);port=int(sys.argv[-1].rsplit(':',1)[1])
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  value=json.dumps({'pid':os.getpid(),'home':os.environ['HOME'],'retained':(state/'retained.txt').read_text()}).encode()
  self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(value)
 def log_message(self,*args): pass
http.server.HTTPServer(('127.0.0.1',port),Handler).serve_forever()
''');binary.chmod(0o700)
    label='com.cuttlefish.app.'+hashlib.sha256(str(state).encode()).hexdigest()[:32]
    target=f'gui/{os.getuid()}/{label}'
    def action(name):
        subprocess.run(['sh',str(SCRIPT),name,str(state),str(binary),f'127.0.0.1:{port}','127.0.0.1',str(port),str(home)],check=True,timeout=90)
    def status():
        with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/local/status',timeout=2) as response:return json.load(response)
    try:
        action('install');first=status();assert first['home']==str(home)
        action('install');assert status()['pid']==first['pid'],'repeated install must not restart'
        os.kill(first['pid'],signal.SIGKILL)
        restarted=wait_for(lambda:(v if (v:=status())['pid']!=first['pid'] else None),'launchd crash restart')
        assert restarted['retained']=='retain across restarts'
        action('restart');assert status()['pid']!=restarted['pid']
        action('stop');assert subprocess.run(['launchctl','print',target],capture_output=True).returncode!=0
        action('start');assert status()['retained']=='retain across restarts'
        action('status');action('uninstall');assert marker.read_text()=='retain across restarts'
        assert not list((home/'Library/LaunchAgents').glob('*.plist'))
        print('PASS: real launchd install, idempotency, crash recovery, restart, stop/start, and uninstall')
    except BaseException:
        result=subprocess.run(['launchctl','print',target],capture_output=True,text=True)
        print(result.stdout, result.stderr, file=sys.stderr)
        error_log=state/'service/stderr.log'
        if error_log.exists(): print(error_log.read_text()[-8000:],file=sys.stderr)
        raise
    finally:
        subprocess.run(['launchctl','bootout',target],capture_output=True)
