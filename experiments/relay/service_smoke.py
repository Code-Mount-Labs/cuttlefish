"""Exercise native service-manager commands without touching real services."""
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[2] / 'install/relay.sh'
IDENTITY = '11111111-1111-4111-8111-111111111111'
LABEL = 'com.cuttlefish.relay.' + IDENTITY + '.epoch.8080'
MOCK = '''#!/usr/bin/env python3
import json,os,pathlib,sys
name=pathlib.Path(sys.argv[0]).name
args=sys.argv[1:]
root=pathlib.Path(os.environ['MOCK_ROOT'])
with (root/'calls').open('a') as file:file.write(json.dumps([name,*args])+'\\n')
active=root/'active'
if name=='uname':print(os.environ['MOCK_OS'] if args==['-s'] else 'arm64')
elif name=='id':print('501')
elif name=='launchctl':
 if args[0]=='print':sys.exit(0 if args[1]=='gui/501' or active.exists() else 1)
 elif args[0]=='bootstrap':active.touch()
 elif args[0]=='bootout':active.unlink(missing_ok=True)
elif name=='systemctl':
 if args[0]=='--user':args=args[1:]
 if os.environ.get('BROKEN_BUS')=='1':sys.exit(1)
 if args[0]=='show':print('loaded' if active.exists() else 'not-found')
 elif args[0]=='enable':active.touch()
 elif args[0]=='disable':active.unlink(missing_ok=True)
'''

class Services(unittest.TestCase):
    def exercise(self, os_name):
        with tempfile.TemporaryDirectory(prefix='cuttlefish-service-test-') as temporary:
            root=pathlib.Path(temporary);binary=root/'bin';binary.mkdir()
            for name in ['uname','id','launchctl','systemctl']:
                path=binary/name;path.write_text(MOCK);path.chmod(0o700)
            env=dict(os.environ,PATH=str(binary)+os.pathsep+os.environ['PATH'],MOCK_ROOT=str(root),MOCK_OS=os_name)
            state=root/'installation';state.mkdir();(state/'frp-0.72.0').mkdir()
            frpc=state/'frp-0.72.0/frpc';frpc.write_text('#!/bin/sh\nexit 0\n');frpc.chmod(0o700)
            ext='plist' if os_name=='Darwin' else 'service'
            (state/(LABEL+'.'+ext)).write_text('fixture')
            foreign='com.cuttlefish.relay.22222222-2222-4222-8222-222222222222.epoch.8080'
            (state/(foreign+'.'+ext)).write_text('foreign fixture')
            def run(action, environment=env):
                return subprocess.run(['sh',str(SCRIPT),action,str(state),IDENTITY,LABEL],env=environment,capture_output=True).returncode
            self.assertEqual(run('start'),0);self.assertTrue((root/'active').exists())
            self.assertEqual(run('start'),0)
            if os_name=='Linux':
                self.assertNotEqual(run('stop',dict(env,BROKEN_BUS='1')),0)
                self.assertTrue((root/'active').exists())
            self.assertEqual(run('stop'),0);self.assertFalse((root/'active').exists())
            self.assertEqual(run('stop'),0)
            calls=[json.loads(line) for line in (root/'calls').read_text().splitlines()]
            self.assertFalse(any(foreign in str(call) for call in calls))
            if os_name=='Darwin':self.assertEqual(sum('bootstrap' in call for call in calls),1)
            self.assertNotEqual(run('invalid'),0)
    def test_launchd_idempotence_and_owned_withdrawal(self):self.exercise('Darwin')
    def test_systemd_idempotence_and_unavailable_bus(self):self.exercise('Linux')

if __name__=='__main__':unittest.main()
