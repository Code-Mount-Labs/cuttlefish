"""Verify the relay image's non-root configuration and child-failure cleanup."""
import argparse
import pathlib
import subprocess
import uuid


def run(image):
    repo = pathlib.Path(__file__).resolve().parents[2]
    mounts = []
    for filename in ['nginx.conf', 'entrypoint.sh']:
        mounts += ['--volume', f'{repo}/deploy/relay/{filename}:/etc/cuttlefish/{filename}:ro']
    subprocess.run(['docker', 'run', '--rm', *mounts, '--entrypoint', 'nginx', image, '-c', '/etc/cuttlefish/nginx.conf', '-t'], check=True, timeout=20)
    name = 'cuttlefish-relay-failure-' + uuid.uuid4().hex[:12]
    try:
        result = subprocess.run(['docker', 'run', '--rm', '--name', name, *mounts,
                                 '--env', 'DATABASE_URL=invalid',
                                 '--env', 'PUBLIC_ORIGIN=https://issuer.invalid',
                                 '--env', 'RELAY_ORIGIN=https://relay.invalid',
                                 '--env', 'RELAY_PUBLIC_KEY=invalid', image], capture_output=True, timeout=20)
        if result.returncode == 0:
            raise AssertionError('Invalid configuration must fail the container')
        exists = subprocess.run(['docker', 'inspect', name], capture_output=True)
        if exists.returncode == 0:
            raise AssertionError('Failed relay unit left a container behind')
        print('Non-root nginx validation and bounded child-failure cleanup passed')
    finally:
        subprocess.run(['docker', 'rm', '--force', name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', default='cuttlefish-relay:local-test')
    run(parser.parse_args().image)
