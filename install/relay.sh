#!/bin/sh
# Runs noninteractively under the installation owner. Never receives a secret argument.
set -eu
umask 077
action=$1
root=$2
installation=$3
selected=$4
case "$installation" in ''|*[!a-f0-9-]*) exit 2;; esac
case "$action" in start|stop) ;; *) exit 2;; esac
os=$(uname -s)
uid=$(id -u)
case "$os" in
  Darwin) extension=plist; launchctl print "gui/$uid" >/dev/null 2>&1 ;;
  Linux) extension=service ;;
  *) exit 2 ;;
esac
manager() {
  if [ "$uid" = 0 ]; then systemctl "$@"; else systemctl --user "$@"; fi
}
# Each manifest belongs to this installation and this grant epoch. A restarted
# supervisor finds the same service instead of creating another process.
for manifest in "$root"/com.cuttlefish.relay."$installation".*."$extension"; do
  [ -f "$manifest" ] || continue
  label=$(basename "$manifest" ".$extension")
  [ "$action" = start ] && [ "$label" = "$selected" ] && continue
  if [ "$os" = Darwin ]; then
    if launchctl print "gui/$uid/$label" >/dev/null 2>&1; then
      launchctl bootout "gui/$uid/$label"
    fi
  else
    # A disabled service can still be active. Stop before removing enablement.
    if state=$(manager show "$label.service" --property=LoadState --value 2>/dev/null); then
      case "$state" in
        loaded) manager disable --now "$label.service" ;;
        not-found) ;;
        *) exit 1 ;;
      esac
    else
      [ "$state" = not-found ] || exit 1
    fi
  fi
done
[ "$action" = start ] || exit 0
[ -f "$root/$selected.$extension" ] || exit 2
binary="$root/frp-0.72.0/frpc"
if [ ! -x "$binary" ]; then
  arch=$(uname -m)
  case "$os/$arch" in
    Darwin/arm64) target=darwin_arm64; digest=6e8576c5b6b9914a86850f6ff4fb81c5a09e706f91f70f64d5f398558bdc9db8 ;;
    Darwin/x86_64) target=darwin_amd64; digest=60439dc6bf794c4182a1a4cac147d642f839d55201607208979eb23aecd09be9 ;;
    Linux/aarch64|Linux/arm64) target=linux_arm64; digest=02180b857480302fd934e9ddbd50fe2f73229f99062f88756abc6de9df5e95cf ;;
    Linux/x86_64) target=linux_amd64; digest=daf719b32ca2778770bb92955e231a07414d754378b9af328aa2642862479804 ;;
    *) exit 2 ;;
  esac
  staging=$(mktemp -d "$root/download.XXXXXX")
  trap 'rm -rf "$staging"' EXIT HUP INT TERM
  archive="frp_0.72.0_$target"
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --connect-timeout 15 --max-time 180 "https://github.com/fatedier/frp/releases/download/v0.72.0/$archive.tar.gz" -o "$staging/archive.tar.gz"
  if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$staging/archive.tar.gz"); else actual=$(shasum -a 256 "$staging/archive.tar.gz"); fi
  [ "${actual%% *}" = "$digest" ] || exit 3
  tar -xzf "$staging/archive.tar.gz" -C "$staging" "$archive/frpc"
  mkdir -p "$root/frp-0.72.0"
  chmod 700 "$staging/$archive/frpc"
  mv "$staging/$archive/frpc" "$binary"
fi
if [ "$os" = Darwin ]; then
  if ! launchctl print "gui/$uid/$selected" >/dev/null 2>&1; then
    launchctl bootstrap "gui/$uid" "$root/$selected.plist"
  fi
else
  manager link "$root/$selected.service"
  manager daemon-reload
  manager enable --now "$selected.service"
fi
