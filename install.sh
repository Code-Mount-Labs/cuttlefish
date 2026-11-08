#!/bin/sh
# Public bootstrap: verified release binaries; no Rust toolchain required.
set -eu
version=${CUTTLEFISH_VERSION:-v0.2.0-alpha.6}
case "$version" in *[!a-zA-Z0-9._-]*|'') echo 'Invalid release version.' >&2; exit 1;; esac
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-musl;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl;;
  Darwin-arm64) target=aarch64-apple-darwin;;
  Darwin-x86_64) target=x86_64-apple-darwin;;
  *) echo 'Unsupported platform. This release supports Linux and macOS; Windows installation is deferred.' >&2; exit 1;;
esac
root=${CUTTLEFISH_INSTALL_DIR:-"$HOME/.local/share/cuttlefish"}
asset="cuttlefish-$version-$target.tar.gz"
base="https://github.com/Code-Mount-Labs/cuttlefish/releases/download/$version"
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
curl --proto '=https' --tlsv1.2 -fsSL "$base/$asset" -o "$temporary/$asset"
curl --proto '=https' --tlsv1.2 -fsSL "$base/SHA256SUMS" -o "$temporary/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {print $1}' "$temporary/SHA256SUMS")
case "$expected" in *[!a-f0-9]*|'') echo 'Missing or invalid release checksum.' >&2; exit 1;; esac
[ "${#expected}" -eq 64 ] || exit 1
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$temporary/$asset" | awk '{print $1}'); else actual=$(shasum -a 256 "$temporary/$asset" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || { echo 'Release checksum verification failed.' >&2; exit 1; }
if tar -tzf "$temporary/$asset" | awk '/^\// || /(^|\/)\.\.($|\/)/ {bad=1} END {exit !bad}'; then echo 'Invalid archive paths.' >&2; exit 1; fi
mkdir "$temporary/unpack"
tar -xzf "$temporary/$asset" -C "$temporary/unpack"
mkdir -p "$root/bin"
for name in cuttlefish orchestrator-node orchestrator-coordinator; do
  [ -f "$temporary/unpack/$name" ] && [ ! -L "$temporary/unpack/$name" ] || exit 1
  pending=$(mktemp "$root/bin/.$name.XXXXXX")
  cp "$temporary/unpack/$name" "$pending"
  chmod 0755 "$pending"
  mv "$pending" "$root/bin/$name"
done
# Defaults are versioned separately; the setup command preserves edited local prompts.
mkdir -p "$root/prompts"
cp -R "$temporary/unpack/prompts/." "$root/prompts/"
printf 'Installed Cuttlefish %s in %s/bin\n' "$version" "$root"
printf 'For future commands use %s/bin/cuttlefish or add that directory to PATH.\n' "$root"
if [ "${CUTTLEFISH_NO_SETUP:-0}" != 1 ]; then
  "$root/bin/cuttlefish" setup "$@" </dev/tty
fi
