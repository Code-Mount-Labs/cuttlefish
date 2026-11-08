#!/usr/bin/env python3
"""Isolated installer upgrades: atomic activation, legacy migration and data safety."""
import hashlib
import io
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[2] / "install.sh"

class UpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cuttlefish-update-test-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "application with spaces"
        self.home = self.base / "home"
        self.home.mkdir()
        self.bin = self.base / "bin"
        self.bin.mkdir()
        curl = self.bin / "curl"
        curl.write_text("#!/bin/sh\nset -eu\nfor arg do case \"$arg\" in https://*) name=${arg##*/};; esac; done\nwhile [ \"$1\" != -o ]; do shift; done\ncp \"$FIXTURE/$name\" \"$2\"\n")
        curl.chmod(0o755)
        arch = "aarch64" if platform.machine() in ("arm64", "aarch64") else "x86_64"
        target = arch + ("-apple-darwin" if platform.system() == "Darwin" else "-unknown-linux-musl")
        self.asset = self.base / f"cuttlefish-v0.2.0-alpha.7-{target}.tar.gz"
        self.env = dict(os.environ, HOME=str(self.home), SHELL="/bin/zsh", FIXTURE=str(self.base),
            PATH=str(self.bin) + os.pathsep + os.environ["PATH"], CUTTLEFISH_VERSION="v0.2.0-alpha.7",
            CUTTLEFISH_NO_SETUP="1", CUTTLEFISH_INSTALL_DIR=str(self.root))
        self.archive()

    def archive(self, extra=None, omit=None):
        with tarfile.open(self.asset, "w:gz") as archive:
            for name in ("cuttlefish", "orchestrator-node", "orchestrator-coordinator", "prompts/router/selection.md"):
                if name == omit:
                    continue
                data = b"#!/bin/sh\necho cuttlefish-machine 0.2.0-alpha.7\n" if not name.startswith("prompts") else b"default prompt"
                item = tarfile.TarInfo(name)
                item.size, item.mode = len(data), 0o755
                archive.addfile(item, io.BytesIO(data))
            if extra:
                archive.addfile(extra, io.BytesIO(b""))
        (self.base / "SHA256SUMS").write_text(hashlib.sha256(self.asset.read_bytes()).hexdigest()+"  "+self.asset.name+"\n")

    def run_installer(self, success=True):
        result = subprocess.run(["sh", str(INSTALLER)], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stderr)

    def test_updates_preserve_state_custom_prompts_and_previous_release(self):
        state = self.home / ".cuttlefish"
        state.mkdir()
        (state / "setup.sqlite").write_bytes(b"private database")
        (self.home / ".provider-sign-in").write_bytes(b"private provider identity")
        self.run_installer()
        old = (self.root / "current").resolve()
        (self.root / "prompts").unlink()
        (self.root / "prompts").mkdir()
        (self.root / "prompts/custom.md").write_text("edited prompt")
        self.run_installer()
        self.assertNotEqual(old, (self.root / "current").resolve())
        self.assertTrue((old / "cuttlefish").is_file())
        for name in ("cuttlefish", "orchestrator-node", "orchestrator-coordinator"):
            self.assertTrue((self.root / "bin" / name).is_symlink())
            self.assertEqual((self.root / "bin" / name).resolve().parent, (self.root / "current").resolve())
        self.assertEqual((state / "setup.sqlite").read_bytes(), b"private database")
        self.assertEqual((self.home / ".provider-sign-in").read_bytes(), b"private provider identity")
        self.assertEqual((self.root / "prompts/custom.md").read_text(), "edited prompt")
        self.assertFalse((self.home / ".zshrc").exists())

    def test_legacy_install_migrates_without_discarding_original_binaries(self):
        (self.root / "bin").mkdir(parents=True)
        for name in ("cuttlefish", "orchestrator-node", "orchestrator-coordinator"):
            (self.root / "bin" / name).write_text("original "+name)
        self.run_installer()
        legacy = list((self.root / "releases").glob("legacy.*"))
        self.assertEqual(len(legacy), 1)
        self.assertEqual((legacy[0] / "cuttlefish").read_text(), "original cuttlefish")

    def test_rejected_release_leaves_current_version_active(self):
        self.run_installer()
        old = (self.root / "current").resolve()
        (self.base / "SHA256SUMS").write_text("0"*64+"  "+self.asset.name+"\n")
        self.run_installer(False)
        self.assertEqual(old, (self.root / "current").resolve())
        self.archive(omit="orchestrator-node")
        self.run_installer(False)
        self.assertEqual(old, (self.root / "current").resolve())

    def test_activation_failure_leaves_all_entrypoints_on_previous_release(self):
        self.run_installer()
        old = (self.root / "current").resolve()
        mv = self.bin / "mv"
        mv.write_text('#!/bin/sh\nfor last do :; done\ncase "$last" in */current) exit 1;; esac\nexec /bin/mv "$@"\n')
        mv.chmod(0o755)
        self.run_installer(False)
        for name in ("cuttlefish", "orchestrator-node", "orchestrator-coordinator"):
            self.assertEqual((self.root / "bin" / name).resolve().parent, old)

    def test_archive_links_and_traversal_are_rejected_before_extraction(self):
        sentinel = self.base / "sentinel"
        sentinel.write_text("unchanged")
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.REGTYPE):
            item = tarfile.TarInfo("../sentinel" if kind == tarfile.REGTYPE else "unsafe")
            item.type, item.linkname = kind, str(sentinel)
            self.archive(extra=item)
            self.run_installer(False)
            self.assertEqual(sentinel.read_text(), "unchanged")
            self.assertFalse(self.root.exists())

    def test_default_install_adds_one_command_path_without_rewriting_profile(self):
        del self.env["CUTTLEFISH_INSTALL_DIR"]
        profile = self.home / ".zshrc"
        profile.write_text("# existing settings\n")
        self.run_installer()
        self.run_installer()
        self.assertTrue((self.home / ".local/bin/cuttlefish").is_file())
        self.assertEqual(profile.read_text().count("# Cuttlefish command path"), 1)
        self.assertTrue(profile.read_text().startswith("# existing settings\n"))

if __name__ == "__main__":
    unittest.main()
