"""Publish only a completed, CI-verified installer run from this repository."""
import base64
import hashlib
import json
import os
import pathlib
import re
import subprocess
import tempfile
import tomllib


def gh(*args):
    return subprocess.check_output(['gh', *args], text=True).strip()


def api(path):
    return json.loads(gh('api', 'repos/' + repository + '/' + path))


repository = os.environ['GITHUB_REPOSITORY']
run_id = os.environ['VERIFIED_RUN_ID']
tag = os.environ['PUBLISH_TAG']
if not re.fullmatch(r'[0-9]+', run_id):
    raise ValueError('A numeric build run ID is required')
if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+', tag):
    raise ValueError('Only versioned alpha releases are supported')
run = api('actions/runs/' + run_id)
assert run['conclusion'] == 'success' and run['status'] == 'completed'
assert run['path'] == '.github/workflows/release-build.yml'
assert run['repository']['full_name'] == repository
source = run['head_sha']
assert re.fullmatch(r'[a-f0-9]{40}', source)
manifest = api('contents/Cargo.toml?ref=' + source)
version = tomllib.loads(base64.b64decode(manifest['content']).decode())['workspace']['package']['version']
assert tag == 'v' + version
required_jobs = {'React frontend', 'Full source coverage', 'Test (ubuntu-latest)', 'Test (macos-latest)', 'Test (windows-latest)'}
checks = api('actions/workflows/ci.yml/runs?head_sha=' + source + '&status=success&per_page=100')['workflow_runs']
assert checks, 'No passing Rust CI run for the exact build source'
verified = False
for check in checks:
    jobs = api('actions/runs/' + str(check['id']) + '/jobs?per_page=100')['jobs']
    passed = {job['name'] for job in jobs if job['conclusion'] == 'success'}
    if required_jobs <= passed:
        verified = True
        break
assert verified, 'Full source coverage and all OS checks must pass'
targets = ['aarch64-apple-darwin', 'x86_64-apple-darwin', 'aarch64-unknown-linux-musl', 'x86_64-unknown-linux-musl']
with tempfile.TemporaryDirectory(prefix='cuttlefish-release-') as folder:
    root = pathlib.Path(folder)
    gh('run', 'download', run_id, '--repo', repository, '--pattern', 'installer-*', '--dir', str(root))
    assets = []
    hashes = {}
    for target in targets:
        name = f'cuttlefish-{tag}-{target}.tar.gz'
        archive = root / ('installer-' + target) / name
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        assert (archive.parent / ('SHA256SUMS-' + target)).read_text().strip() == digest + '  ' + name
        hashes[name] = digest
        assets.append(str(archive))
    sums = root / 'SHA256SUMS'
    sums.write_text(''.join(f'{digest}  {name}\n' for name, digest in sorted(hashes.items())))
    assets.append(str(sums))
    hashes[sums.name] = hashlib.sha256(sums.read_bytes()).hexdigest()
    notes = root / 'notes.md'
    notes.write_text(f'''Cuttlefish {tag} includes the new chat dashboard, scheduled Routes, reusable Workflows, and local Codex/Claude Code providers over managed HTTPS. It fixes completed workflow sessions occupying worker capacity: durable cleanup now releases each workflow session while preserving its results.

Create a group at https://platform-production-105c.up.railway.app, choose **Connect a machine**, then run:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/{repository}/{tag}/install.sh | sh
```

Choose **managed**, **local**, or an existing HTTPS address. Complete first-time administrator setup locally before managed access activates.

Includes the selected Mira/cyan shadcn preset with locally bundled fonts, attachments and artifacts, provider progress, durable workflow runs, and scheduled task recovery. Existing provider sign-ins stay on the execution machine. Installation-scoped relay authorization and customer-owned PostgreSQL remain separate from the hosted directory database.

To update an existing installation without starting setup again, run the same installer with `CUTTLEFISH_NO_SETUP=1`, then restart Cuttlefish using its existing state directory. Back up the state directory and execution database before updating.

Linux and macOS: ARM64 and x86-64. All archives are checksum-verified. [Build verification]({run['html_url']}) and [Rust CI]({check['html_url']}) passed, including the unchanged 100% line, function and region coverage gate.

This alpha uses Cuttlefish staging. The main process runs in the foreground; the connector has its own OS service. Public relay access supports JSON polling; streaming/WebSockets and Windows installation remain deferred. Routes use elapsed intervals rather than calendar cron; Plugins is reserved for future integrations.
''')
    release = next((item for item in api('releases?per_page=100') if item['tag_name'] == tag), None)
    if release is None:
        gh('release', 'create', tag, *assets, '--repo', repository, '--draft', '--prerelease', '--target', source, '--title', 'Cuttlefish ' + tag, '--notes-file', str(notes))
        release = next(item for item in api('releases?per_page=100') if item['tag_name'] == tag)
    # Draft tags are not available through releases/tags until publication.
    # Resume only an exact draft; never replace an existing published release.
    assert release['draft'] and release['target_commitish'] == source
    assert len(release['assets']) == len(hashes)
    for asset in release['assets']:
        assert asset['digest'] == 'sha256:' + hashes[asset['name']], 'Uploaded asset checksum mismatch'
    gh('release', 'edit', tag, '--repo', repository, '--draft=false', '--latest=false')
    print('Published', tag, 'from verified source', source)
