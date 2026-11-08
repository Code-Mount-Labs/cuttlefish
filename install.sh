#!/bin/sh
# Public bootstrap: verified release binaries; no Rust toolchain required.
set -eu
version=${CUTTLEFISH_VERSION:-v0.2.0-alpha.9}
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
trap 'rm -rf "$temporary"' EXIT
trap 'exit 1' HUP INT TERM
curl --proto '=https' --tlsv1.2 -fsSL "$base/$asset" -o "$temporary/$asset"
curl --proto '=https' --tlsv1.2 -fsSL "$base/SHA256SUMS" -o "$temporary/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {print $1}' "$temporary/SHA256SUMS")
case "$expected" in *[!a-f0-9]*|'') echo 'Missing or invalid release checksum.' >&2; exit 1;; esac
[ "${#expected}" -eq 64 ] || exit 1
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$temporary/$asset" | awk '{print $1}'); else actual=$(shasum -a 256 "$temporary/$asset" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || { echo 'Release checksum verification failed.' >&2; exit 1; }
if tar -tzf "$temporary/$asset" | awk '/^\// || /(^|\/)\.\.($|\/)/ {bad=1} END {exit !bad}'; then echo 'Invalid archive paths.' >&2; exit 1; fi
# Releases contain regular files and directories only. Reject links and special
# files before extraction, including links that could escape the staging folder.
if tar -tvzf "$temporary/$asset" | awk 'substr($0,1,1) != "-" && substr($0,1,1) != "d" {bad=1} END {exit !bad}'; then echo 'Invalid archive entry types.' >&2; exit 1; fi
mkdir "$temporary/unpack"
tar --no-same-owner --no-same-permissions -xzf "$temporary/$asset" -C "$temporary/unpack"
# Immutable release directories make activation one atomic pointer replacement.
# Existing application state and provider credentials are never part of a release.
mkdir -p "$root/bin" "$root/releases"
root=$(cd "$root" && pwd -P)
for name in cuttlefish orchestrator-node orchestrator-coordinator; do
  [ -f "$temporary/unpack/$name" ] && [ ! -L "$temporary/unpack/$name" ] || exit 1
  chmod 0755 "$temporary/unpack/$name"
done
[ "$("$temporary/unpack/cuttlefish" --version)" = "cuttlefish-machine ${version#v}" ] || {
  echo 'Release executable version check failed.' >&2; exit 1;
}
atomic_link() {
  link_source=$1
  link_target=$2
  pending=$(mktemp "$root/.link.XXXXXX")
  rm "$pending"
  ln -s "$link_source" "$pending"
  case "$(uname -s)" in
    Darwin) mv -f -h "$pending" "$link_target";;
    *) mv -f -T "$pending" "$link_target";;
  esac
}
# Convert legacy installs without mixing old and new binaries. Until all stable
# entry points exist, `current` still selects a complete copy of the old release.
if [ ! -L "$root/current" ] && [ -f "$root/bin/cuttlefish" ]; then
  legacy=$(mktemp -d "$root/releases/legacy.XXXXXX")
  for name in cuttlefish orchestrator-node orchestrator-coordinator; do
    [ -f "$root/bin/$name" ] || { echo 'Existing install is incomplete.' >&2; exit 1; }
    cp -p "$root/bin/$name" "$legacy/$name"
  done
  atomic_link "releases/$(basename "$legacy")" "$root/current"
fi
release=$(mktemp -d "$root/releases/$version-$target.XXXXXX")
cp -R "$temporary/unpack/." "$release/"
printf '%s\n' "$actual" > "$release/.archive-sha256"
printf '%s\n' "$version" > "$release/.release-version"
# A first install can activate now because there are no old entry points.
if [ ! -L "$root/current" ]; then
  [ ! -e "$root/current" ] || { echo 'Unrecognized current release path.' >&2; exit 1; }
  atomic_link "releases/$(basename "$release")" "$root/current"
fi
for name in cuttlefish orchestrator-node orchestrator-coordinator; do
  atomic_link "../current/$name" "$root/bin/$name"
done
printf 'cuttlefish-release-layout-v1\n' > "$root/.cuttlefish-layout"
atomic_link "releases/$(basename "$release")" "$root/current"
# Preserve any existing defaults folder; runtime-edited prompts live in state.
if [ ! -e "$root/prompts" ]; then atomic_link 'current/prompts' "$root/prompts"; fi
# Normal installs make `cuttlefish update` available in the next terminal.
# Explicit custom/test directories never alter the account's command path.
if [ -z "${CUTTLEFISH_INSTALL_DIR+x}" ]; then
  mkdir -p "$HOME/.local/bin"
  shortcut="$HOME/.local/bin/cuttlefish"
  if [ ! -e "$shortcut" ] && [ ! -L "$shortcut" ]; then ln -s "$root/bin/cuttlefish" "$shortcut"; fi
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *)
      case "${SHELL:-}" in */zsh) profile="$HOME/.zshrc";; */bash) profile="$HOME/.bashrc";; *) profile="$HOME/.profile";; esac
      if ! grep -Fq '# Cuttlefish command path' "$profile" 2>/dev/null; then
        printf '\n# Cuttlefish command path\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$profile"
      fi
      printf 'Open a new terminal to use the cuttlefish command.\n'
      ;;
  esac
fi
printf 'Installed Cuttlefish %s in %s/bin\n' "$version" "$root"
printf 'For future commands use %s/bin/cuttlefish or add that directory to PATH.\n' "$root"
if [ "${CUTTLEFISH_NO_SETUP:-0}" != 1 ]; then
  "$root/bin/cuttlefish" setup "$@" </dev/tty
fi
