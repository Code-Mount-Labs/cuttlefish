#!/usr/bin/env python3
"""Test the real bootstrap with release bytes supplied by a local HTTP fixture.

Only the download transport is substituted. Checksum verification, extraction,
installation, executable smoke checks, and repeat installation run unmodified.
"""
import hashlib
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile


def exercise(archive, installer, destination):
    version = re.fullmatch(r"cuttlefish-(v.+)-(?:aarch64|x86_64)-.+\.tar\.gz", archive.name)[1]
    with tempfile.TemporaryDirectory(prefix="cuttlefish-bootstrap-") as folder:
        fixture = pathlib.Path(folder)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        shutil.copyfile(archive, fixture / archive.name)
        manifest = fixture / "SHA256SUMS"
        manifest.write_text(f"{digest}  {archive.name}\n")
        bin_dir = fixture / "bin"
        bin_dir.mkdir()
        downloader = bin_dir / "curl"
        downloader.write_text("#!" + sys.executable + "\n" + '''
import os,pathlib,shutil,sys
args=sys.argv[1:]
base="https://github.com/Code-Mount-Labs/cuttlefish/releases/download/"+os.environ["FIXTURE_VERSION"]+"/"
url=next(value for value in args if value.startswith("https://"))
assert url.startswith(base), "unexpected download origin"
name=url[len(base):]
assert "/" not in name and name in (os.environ["FIXTURE_ARCHIVE"],"SHA256SUMS")
shutil.copyfile(pathlib.Path(os.environ["FIXTURE_ROOT"])/name,args[args.index("-o")+1])
''')
        downloader.chmod(0o755)
        env = dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"],
                   CUTTLEFISH_INSTALL_DIR=str(destination), CUTTLEFISH_NO_SETUP="1",
                   FIXTURE_VERSION=version, FIXTURE_ROOT=str(fixture), FIXTURE_ARCHIVE=archive.name)
        for invalid in ["0" * 64, "not-a-checksum"]:
            manifest.write_text(f"{invalid}  {archive.name}\n")
            rejected = subprocess.run(["sh", str(installer)], env=env, capture_output=True)
            assert rejected.returncode != 0 and not destination.exists()
        manifest.write_text(f"{digest}  {archive.name}\n")
        for _ in range(2):
            subprocess.run(["sh", str(installer)], env=env, check=True)
            for name in ["cuttlefish", "orchestrator-node", "orchestrator-coordinator"]:
                subprocess.run([str(destination / "bin" / name), "--help"], check=True,
                               stdout=subprocess.DEVNULL)
        assert (destination / "prompts/router/selection.md").is_file()
        print("PASS: bootstrap rejects bad checksums, installs all executables and prompts, and repeats safely")


if __name__ == "__main__":
    exercise(*(pathlib.Path(value).resolve() for value in sys.argv[1:]))
