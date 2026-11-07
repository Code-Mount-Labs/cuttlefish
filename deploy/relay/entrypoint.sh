#!/bin/bash
set -euo pipefail
umask 077
children=()
cleanup() {
  trap - EXIT INT TERM
  # Rust installs a SIGINT handler. Background nginx/frps must receive TERM:
  # shells can start asynchronous jobs with SIGINT ignored.
  if (( ${#children[@]} )); then kill -INT "${children[0]}" 2>/dev/null || true; fi
  for child in "${children[@]:1}"; do kill -TERM "$child" 2>/dev/null || true; done
  ( sleep 10; for child in "${children[@]}"; do kill -KILL "$child" 2>/dev/null || true; done ) &
  deadline=$!
  for child in "${children[@]}"; do wait "$child" 2>/dev/null || true; done
  kill -TERM "$deadline" 2>/dev/null || true
  wait "$deadline" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 0' INT TERM
cuttlefish-relay &
children+=("$!")
frps -c /etc/cuttlefish/frps.toml &
children+=("$!")
nginx -c /etc/cuttlefish/nginx.conf -g 'daemon off;' &
children+=("$!")
# Any child exit ends the whole unit; Railway restarts the service.
wait -n "${children[@]}" || exit 1
exit 1
