#!/bin/sh
# One systemd supervisor per saved installation. Never changes another service.
set -eu
[ "$(uname -s)" = Linux ] || { echo 'This service adapter requires Linux.' >&2; exit 1; }
carriage_return=$(printf '\r')
action=$1; state=$2; binary=$3; listen=$4; host=$5; port=$6; account_home=$7
case "$action" in install|start|stop|restart|status|uninstall) ;; *) exit 1;; esac
for value in "$state" "$binary" "$account_home" "$PATH"; do
  case "$value" in *'
'*|*"$carriage_return"*) echo 'Service paths must not contain line breaks.' >&2; exit 1;; esac
done
[ -d "$state" ] && [ -f "$state/setup.sqlite" ] || { echo 'Complete Cuttlefish setup in this state directory first.' >&2; exit 1; }
[ -x "$binary" ] || { echo 'The installed Cuttlefish executable is unavailable.' >&2; exit 1; }
for tool in systemctl curl ss sha256sum /usr/bin/env; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Install $tool before configuring the background service (ss is provided by iproute2)." >&2; exit 1; }
done
uid=$(id -u)
if [ "$uid" = 0 ]; then scope=system; target=multi-user.target; else scope=user; target=default.target; fi
manager() { if [ "$scope" = system ]; then systemctl "$@"; else systemctl --user "$@"; fi; }
manager show-environment >/dev/null || { echo 'systemd is unavailable for this account. Use setup --foreground, or enable the user systemd manager.' >&2; exit 1; }
label="cuttlefish-$(printf '%s' "$state" | sha256sum | cut -c 1-32).service"
folder="$state/service"; unit="$folder/$label"; temporary=''
trap '[ -z "$temporary" ] || rm -f "$temporary"' EXIT
trap 'exit 1' HUP INT TERM
# ExecStart/Environment use C-style quoting. The colon command prefix disables
# environment expansion; percent specifiers still need escaping. WorkingDirectory
# is a single raw path, so env --chdir preserves special/trailing characters.
quote() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g'; }
owned() {
  [ ! -L "$unit" ] &&
  grep -Fqx '# Cuttlefish systemd service v1' "$unit" &&
  grep -Fqx "# Cuttlefish scope: $scope" "$unit" &&
  grep -Fqx "# Cuttlefish state: $state" "$unit"
}
[ ! -L "$folder" ] || { echo 'Refusing a symlinked service directory.' >&2; exit 1; }
if [ -e "$unit" ] || [ -L "$unit" ]; then
  owned || { echo 'Refusing an unrecognized service definition.' >&2; exit 1; }
elif [ "$action" != install ]; then
  case "$action" in status|stop|uninstall) echo 'Cuttlefish background service is not installed.'; exit 0;; esac
  echo 'Run cuttlefish service install first.' >&2; exit 1
fi
fragment=$(manager show "$label" --property=FragmentPath --value)
[ -z "$fragment" ] || [ "$fragment" -ef "$unit" ] || { echo 'Another service owns this name.' >&2; exit 1; }
if [ "$action" != install ]; then
  listen=$(sed -n 's/^# Cuttlefish listener: //p' "$unit")
  [ -n "$listen" ] || { echo 'Missing saved service listener.' >&2; exit 1; }
  host=${listen%:*}; host=${host#\[}; host=${host%\]}; port=${listen##*:}
fi
case "$port" in ''|*[!0-9]*) echo 'Invalid service port.' >&2; exit 1;; esac
case "$host" in 0.0.0.0) probe_host=127.0.0.1;; ::) probe_host='[::1]';; *:*) probe_host="[$host]";; *) probe_host=$host;; esac
url="http://$probe_host:$port/api/local/status"
active() { manager is-active --quiet "$label"; }
occupied() {
  sockets=$(ss -H -ltn "sport = :$port") || { echo 'Could not inspect listening ports.' >&2; exit 1; }
  [ -n "$sockets" ]
}
responding() { curl --noproxy '*' -fsS --max-time 2 "$url" >/dev/null 2>&1; }
linger() {
  if [ "$scope" = user ] && [ "$(loginctl show-user "$uid" --property=Linger --value)" != yes ]; then
    loginctl enable-linger "$uid" || { echo "Enable startup outside login sessions with: sudo loginctl enable-linger $uid" >&2; exit 1; }
  fi
}
start_service() {
  if ! active && occupied; then
    echo "Port $port is already in use. Stop the foreground process before starting this service." >&2; exit 1
  fi
  linger
  manager enable --now "$unit"
  attempts=0
  until active && responding; do
    attempts=$((attempts + 1))
    [ "$attempts" -lt 40 ] || { echo "Dashboard not ready. Run cuttlefish service status; inspect journalctl for $label." >&2; exit 1; }
    sleep 0.5
  done
  echo "Cuttlefish is running in the background at http://$probe_host:$port."
  echo 'It starts automatically when this machine boots.'
}
stop_service() {
  manager stop "$label"
  attempts=0
  while occupied; do
    attempts=$((attempts + 1))
    [ "$attempts" -lt 20 ] || { echo 'The previous listener is still closing; retry shortly.' >&2; exit 1; }
    sleep 0.5
  done
}
case "$action" in
 install)
  if ! active && occupied; then echo "Port $port is already in use. Stop the foreground process first." >&2; exit 1; fi
  umask 077
  mkdir -p "$folder"; chmod 700 "$folder"
  temporary=$(mktemp "$folder/.unit.XXXXXX")
  cat > "$temporary" <<EOF
# Cuttlefish systemd service v1
# Cuttlefish scope: $scope
# Cuttlefish state: $state
# Cuttlefish listener: $listen
[Unit]
Description=Cuttlefish installation supervisor
After=network.target
StartLimitIntervalSec=0
[Service]
Type=exec
WorkingDirectory=/
ExecStart=:/usr/bin/env "--chdir=$(quote "$state")" -- "$(quote "$binary")" --state-dir "$(quote "$state")" serve --listen "$listen"
Environment="HOME=$(quote "$account_home")"
Environment="PATH=$(quote "$PATH")"
EOF
  for name in CODEX_HOME CLAUDE_CONFIG_DIR DOCKER_CONFIG DOCKER_HOST XDG_CONFIG_HOME XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS; do
    value=$(printenv "$name" || true)
    case "$value" in *'
'*|*"$carriage_return"*) echo 'Service configuration must not contain line breaks.' >&2; exit 1;; esac
    [ -z "$value" ] || printf 'Environment="%s=%s"\n' "$name" "$(quote "$value")" >> "$temporary"
  done
  cat >> "$temporary" <<EOF
Restart=always
RestartSec=2
KillSignal=SIGINT
# Specialists and the detached updater own durable journals and deliberately
# survive supervisor restarts. Only the supervisor is this unit's main process.
KillMode=process
TimeoutStopSec=20
UMask=0077
StandardOutput=journal
StandardError=journal
[Install]
WantedBy=$target
EOF
  if [ ! -f "$unit" ] || ! cmp -s "$temporary" "$unit"; then
    if active; then stop_service; fi
    mv "$temporary" "$unit"; temporary=''
    manager link "$unit"
    manager daemon-reload
  fi
  start_service;;
 start) start_service;;
 restart) stop_service; start_service;;
 stop|uninstall)
  # Stop while the linked definition still owns KillMode; disable can unload it.
  stop_service
  manager disable "$label"
  if [ "$action" = uninstall ]; then rm "$unit"; manager daemon-reload; fi
  echo 'Cuttlefish automatic startup is disabled. Application data and provider sign-ins are unchanged.';;
 status)
  if active; then
    if responding; then echo 'Cuttlefish background service is active; dashboard is responding.';
    else echo "Cuttlefish is active but the dashboard is not responding. Inspect journalctl for $label."; exit 1; fi
  else echo 'Cuttlefish background service is installed but stopped.'; fi;;
esac
