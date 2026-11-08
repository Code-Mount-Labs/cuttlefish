#!/usr/bin/env python3
"""OS-service contract tests: fake launchd, real shell/filesystem/plist parsing.

Never loads a real service. Temporary homes isolate all startup definitions.
"""
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / 'install/service-macos.sh'
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, plistlib, sys
p=pathlib.Path(os.environ['SERVICE_FIXTURE'])
s=json.loads(p.read_text()); name=pathlib.Path(sys.argv[0]).name; args=sys.argv[1:]
s.setdefault('calls',[]).append([name,*args]); code=0
if name=='uname': print(s.get('os','Darwin'))
elif name=='id': print(s.get('uid',501))
elif name=='sleep': pass
elif name=='plutil':
 try: plistlib.loads(pathlib.Path(args[-1]).read_bytes())
 except Exception: code=1
elif name=='nc': code=0 if s.get('occupied',False) or s.get('loaded',False) else 1
elif name=='curl': code=0 if s.get('loaded',False) and not s.get('unhealthy',False) else 1
elif name=='launchctl':
 op=args[0]
 if op==s.get('fail'): code=1
 elif op=='print': code=0 if s.get('loaded',False) else 1
 elif op=='enable': s['enabled']=True
 elif op=='disable': s['enabled']=False
 elif op=='bootout':
  if not s.get('stuck',False): s['loaded']=False
 elif op=='bootstrap':
  if s.get('bootstrap_failures',0)>0: s['bootstrap_failures']-=1; code=1
  else: s['loaded']=True
 elif op=='kickstart': code=0 if s.get('loaded',False) else 1
 else: code=1
p.write_text(json.dumps(s)); sys.exit(code)
'''

class ServiceContract(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cuttlefish-service-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home & ' account"
        self.state = self.home / 'state & <data>'
        self.state.mkdir(parents=True)
        (self.state/'setup.sqlite').write_text('fixture only')
        self.binary = self.home/'bin with spaces/cuttlefish'
        self.binary.parent.mkdir(); self.binary.write_text('#!/bin/sh\nexit 0\n'); self.binary.chmod(0o700)
        self.mock = self.root/'mocks'; self.mock.mkdir()
        for name in ['uname','id','sleep','plutil','nc','curl','launchctl']:
            p=self.mock/name; p.write_text(FAKE); p.chmod(0o700)
        self.fixture=self.root/'fixture.json'; self.fixture.write_text('{}')
        self.env=os.environ.copy(); self.env.update(SERVICE_FIXTURE=str(self.fixture),PATH=str(self.mock)+os.pathsep+os.environ['PATH'],CODEX_HOME=str(self.home/'codex'),CLAUDE_CONFIG_DIR=str(self.home/'claude'),UNRELATED_SECRET='must-not-be-copied')
    def saved(self): return json.loads(self.fixture.read_text())
    def configure(self, **kwargs):
        s=self.saved();s.update(kwargs);self.fixture.write_text(json.dumps(s))
    def run_action(self, action, success=True, listen='127.0.0.1:65432', home=None):
        host, port=listen.rsplit(':',1);host=host.strip('[]')
        result=subprocess.run(['sh',str(SCRIPT),action,str(self.state),str(self.binary),listen,host,port,str(home or self.home)],env=self.env,capture_output=True,text=True,timeout=30)
        self.assertEqual(result.returncode==0,success,result.stdout+result.stderr)
        return result
    def plist(self): return next((self.home/'Library/LaunchAgents').glob('*.plist'))
    def test_lifecycle_recovery_and_data_preservation(self):
        self.configure(bootstrap_failures=2)
        self.run_action('install')
        path=self.plist(); raw=path.read_bytes(); data=plistlib.loads(raw)
        self.assertEqual(data['ProgramArguments'],[str(self.binary),'--state-dir',str(self.state),'serve','--listen','127.0.0.1:65432'])
        self.assertEqual(data['EnvironmentVariables']['HOME'],str(self.home))
        self.assertEqual(data['EnvironmentVariables']['CODEX_HOME'],self.env['CODEX_HOME'])
        self.assertNotIn('UNRELATED_SECRET',data['EnvironmentVariables'])
        self.assertTrue(data['RunAtLoad']);self.assertTrue(data['KeepAlive']);self.assertTrue(data['AbandonProcessGroup'])
        self.assertEqual(path.stat().st_mode&0o777,0o600)
        self.assertEqual((self.state/'service').stat().st_mode&0o777,0o700)
        self.assertEqual((self.state/'service/stderr.log').stat().st_mode&0o777,0o600)
        self.run_action('install');self.assertEqual(path.read_bytes(),raw)
        for action in ['status','restart','stop','status','start','uninstall','uninstall','status','stop']:
            self.run_action(action,listen='127.0.0.1:8080')
        self.assertFalse(path.exists());self.assertEqual((self.state/'setup.sqlite').read_text(),'fixture only')
        self.assertFalse(self.saved()['loaded']);self.assertFalse(self.saved()['enabled'])
        calls=self.saved()['calls']
        # Restart/start must probe the saved custom port, even with the CLI default.
        self.assertTrue(all('65432' in c[-1] for c in calls if c[0]=='curl'))
        self.assertTrue(all('com.cuttlefish.app.' in ' '.join(c) for c in calls if c[0]=='launchctl'))
    def test_preconditions(self):
        for value in [{'os':'Linux'},{'os':'Darwin','uid':0}]:
            self.configure(**value);self.run_action('install',False)
        self.configure(uid=501)
        self.run_action('invalid',False);self.run_action('start',False)
        (self.state/'setup.sqlite').unlink();self.run_action('install',False)
        (self.state/'setup.sqlite').touch();self.binary.chmod(0o600);self.run_action('install',False)
        self.binary.chmod(0o700);self.run_action('install',False,home=self.home/'bad\npath')
    def test_port_conflict_never_enrolls_or_stops_foreground_app(self):
        self.configure(occupied=True);self.run_action('install',False)
        self.assertFalse(list((self.home/'Library/LaunchAgents').glob('*.plist')))
        self.assertFalse(any(c[0]=='launchctl' and c[1]=='bootout' for c in self.saved()['calls']))
        self.configure(occupied=False);self.run_action('install');self.run_action('stop')
        self.configure(occupied=True);self.run_action('start',False)
    def test_unrecognized_and_symlinked_files_are_not_overwritten(self):
        self.run_action('install');path=self.plist();self.run_action('stop');path.write_text('unrelated')
        self.run_action('install',False);self.run_action('uninstall',False);self.assertEqual(path.read_text(),'unrelated')
        other=self.root/'unrelated';other.write_text('preserve');path.unlink();path.symlink_to(other)
        self.run_action('install',False);self.assertEqual(other.read_text(),'preserve')
    def test_log_symlinks_are_rejected(self):
        target=self.root/'elsewhere';target.mkdir();logs=self.state/'service';logs.symlink_to(target)
        self.run_action('install',False);logs.unlink();logs.mkdir();secret=self.root/'secret';secret.write_text('preserve')
        (logs/'stdout.log').symlink_to(secret);self.run_action('install',False);self.assertEqual(secret.read_text(),'preserve')
    def test_bounded_startup_failures_are_reported(self):
        self.configure(bootstrap_failures=50);self.run_action('install',False)
        self.assertEqual(self.saved()['bootstrap_failures'],40)
        self.configure(bootstrap_failures=0,unhealthy=True);self.run_action('start',False);self.run_action('status',False)
        self.configure(unhealthy=False);self.run_action('status')
        self.configure(fail='bootout');self.run_action('stop',False)
        self.configure(fail=None,stuck=True);self.run_action('restart',False)
    def test_reinstall_updates_only_its_service(self):
        self.run_action('install');self.run_action('install',listen='[::1]:65433')
        self.assertEqual(plistlib.loads(self.plist().read_bytes())['ProgramArguments'][-1],'[::1]:65433')
        self.run_action('status')
    def test_missing_listener_and_delayed_port_release(self):
        self.run_action('install');path=self.plist();raw=path.read_text()
        path.write_text('\n'.join(line for line in raw.splitlines() if not line.startswith('<!-- Cuttlefish listener:')))
        self.run_action('status',False);path.write_text(raw)
        self.configure(occupied=True);self.run_action('restart',False)

if __name__=='__main__': unittest.main()
