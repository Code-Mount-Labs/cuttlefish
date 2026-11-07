#!/bin/sh
# Run from a checkout on a host with Lima and native Linux guest support.
set -eu
distribution=${1:-ubuntu-24.04}
mode=${2:-rootless}
archive=${3:-}
case "$distribution" in ubuntu-24.04|debian-13) ;; *) exit 2;; esac
case "$mode" in rootless|rootful) ;; *) exit 2;; esac
repository=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
results=$(mktemp -d "${TMPDIR:-/tmp}/cuttlefish-fresh.XXXXXX")
name="cuttlefish-qa-$(basename "$results" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9-')"
created=no
cleanup() { if [ "$created" = yes ]; then limactl delete --force "$name" >/dev/null 2>&1 || true; fi; }
trap cleanup EXIT HUP INT TERM
echo "Fresh-install results: $results"
created=yes
limactl start --yes --name="$name" --vm-type=vz --cpus=2 --memory=2 --disk=12 --plain --mount-none --containerd=none "template:$distribution" > "$results/boot.log" 2>&1
guest() { limactl shell --workdir=/tmp "$name" "$@"; }
guest sh -c '! command -v docker; cat /etc/os-release' > "$results/initial.txt"
if [ -n "$archive" ]; then
  archive=$(cd "$(dirname "$archive")" && pwd)/$(basename "$archive")
  limactl copy "$archive" "$name:/tmp/$(basename "$archive")"
  for file in install.sh tests/install/bootstrap.py tests/install/release-smoke.py; do
    limactl copy "$repository/$file" "$name:/tmp/$(basename "$file")"
  done
  guest python3 /tmp/bootstrap.py "/tmp/$(basename "$archive")" /tmp/install.sh /tmp/cuttlefish-installed > "$results/bootstrap.log" 2>&1
  if [ "$mode" = rootful ]; then
    guest sudo python3 /tmp/release-smoke.py /tmp/cuttlefish-installed/bin /tmp/cuttlefish-smoke > "$results/application.log" 2>&1
  else
    guest python3 /tmp/release-smoke.py /tmp/cuttlefish-installed/bin /tmp/cuttlefish-smoke > "$results/application.log" 2>&1
  fi
elif [ "$mode" = rootful ]; then
  guest sudo sh -s -- --non-interactive < "$repository/install/docker.sh" > "$results/install.log" 2>&1
else
  guest sh -s -- --non-interactive < "$repository/install/docker.sh" > "$results/install.log" 2>&1
fi
guest sha256sum /etc/subuid /etc/subgid > "$results/subids-before.txt"
if [ "$mode" = rootful ]; then
  guest sudo sh -s -- --non-interactive < "$repository/install/docker.sh" > "$results/repeat.log" 2>&1
else
  guest sh -s -- --non-interactive < "$repository/install/docker.sh" > "$results/repeat.log" 2>&1
fi
guest sha256sum /etc/subuid /etc/subgid > "$results/subids-after.txt"
cmp "$results/subids-before.txt" "$results/subids-after.txt"
if [ "$mode" = rootful ]; then docker_command='sudo docker'; else docker_command=docker; fi
guest sh -s -- "$docker_command" <<'GUEST'
set -eu
docker_command=$1
$docker_command info --format '{{json .SecurityOptions}}'
$docker_command volume create cuttlefish-fresh-data >/dev/null
$docker_command run --rm --mount type=volume,source=cuttlefish-fresh-data,target=/data alpine:3.22 sh -c 'echo persisted > /data/marker'
GUEST
limactl stop "$name" > "$results/stop.log" 2>&1
limactl start --yes "$name" > "$results/restart.log" 2>&1
guest sh -s -- "$docker_command" <<'GUEST'
set -eu
docker_command=$1
$docker_command info > /dev/null
value=$($docker_command run --rm --mount type=volume,source=cuttlefish-fresh-data,target=/data alpine:3.22 cat /data/marker)
[ "$value" = persisted ]
GUEST
echo "PASS: $distribution $mode: fresh install, idempotent repeat, daemon and volume after reboot" | tee "$results/result.txt"
