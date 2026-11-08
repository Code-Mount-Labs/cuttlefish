#!/bin/sh
# Per-installation login service. Called by `cuttlefish service`, never via sudo.
set -eu
[ "$(uname -s)" = Darwin ] || { echo 'Background services currently require macOS.' >&2; exit 1; }
[ "$(id -u)" != 0 ] || { echo 'Run this command as your normal macOS account, without sudo.' >&2; exit 1; }
action=$1
state=$2
binary=$3
listen=$4
host=$5
port=$6
account_home=$7
case "$action" in install|start|stop|restart|status|uninstall) ;; *) echo 'Unknown service action.' >&2; exit 1;; esac
for value in "$state" "$binary" "$account_home" "$PATH"; do
  case "$value" in *'
'*|*'
'*) echo 'Service paths must not contain line breaks.' >&2; exit 1;; esac
done
[ -d "$state" ] && [ -f "$state/setup.sqlite" ] || { echo 'Complete Cuttlefish setup in this state directory first.' >&2; exit 1; }
[ -x "$binary" ] || { echo 'The installed Cuttlefish executable is unavailable.' >&2; exit 1; }
label="com.cuttlefish.app.$(printf '%s' "$state" | shasum -a 256 | cut -c 1-32)"
domain="gui/$(id -u)"
target="$domain/$label"
agents="$account_home/Library/LaunchAgents"
plist="$agents/$label.plist"
logs="$state/service"
temporary=''
trap '[ -z "$temporary" ] || rm -f "$temporary"' EXIT HUP INT TERM
xml() { printf '%s' "$1" | sed -e 's/\&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g' -e "s/'/\&apos;/g"; }
owned() {
  [ ! -L "$plist" ] &&
  grep -Fqx '<!-- Cuttlefish login service v1 -->' "$plist" &&
  grep -Fqx "<key>Label</key><string>$label</string>" "$plist" &&
  grep -Fqx "<key>WorkingDirectory</key><string>$(xml "$state")</string>" "$plist"
}
loaded() { launchctl print "$target" >/dev/null 2>&1; }
if [ -e "$plist" ] || [ -L "$plist" ]; then
  owned || { echo 'Refusing to change an unrecognized service definition.' >&2; exit 1; }
elif [ "$action" != install ]; then
  case "$action" in status|stop|uninstall) echo 'Cuttlefish background service is not installed.'; exit 0;; esac
  echo 'Run cuttlefish service install first.' >&2; exit 1
fi
if [ "$action" != install ]; then
  listen=$(sed -n 's/^<!-- Cuttlefish listener: \(.*\) -->$/\1/p' "$plist")
  [ -n "$listen" ] || { echo 'Missing saved service listener.' >&2; exit 1; }
  host=${listen%:*}; host=${host#\[}; host=${host%\]}; port=${listen##*:}
fi
case "$host" in 0.0.0.0) probe_host=127.0.0.1;; ::) probe_host='[::1]';; *:*) probe_host="[$host]";; *) probe_host=$host;; esac
url="http://$probe_host:$port/api/local/status"
responding() { curl --noproxy '*' -fsS --max-time 2 "$url" >/dev/null 2>&1; }
stop_loaded() {
  if loaded; then
    launchctl bootout "$target" || { echo 'Could not stop the Cuttlefish service.' >&2; exit 1; }
    attempts=0
    while loaded; do
      attempts=$((attempts + 1))
      [ "$attempts" -lt 20 ] || { echo 'Service shutdown is still pending; retry shortly.' >&2; exit 1; }
      sleep 0.5
    done
    attempts=0
    while nc -z -G 1 "$host" "$port" >/dev/null 2>&1; do
      attempts=$((attempts + 1))
      [ "$attempts" -lt 20 ] || { echo 'The previous listener is still closing; retry shortly.' >&2; exit 1; }
      sleep 0.5
    done
  fi
}
start_loaded() {
  launchctl enable "$target"
  if ! loaded; then
    # Never take over a port or terminate a foreground/unrelated installation.
    if nc -z -G 1 "$host" "$port" >/dev/null 2>&1; then
      echo "Port $port is already in use. Stop the foreground Cuttlefish process, then retry." >&2
      exit 1
    fi
    attempts=0
    until launchctl bootstrap "$domain" "$plist" >/dev/null 2>&1; do
      loaded && break
      attempts=$((attempts + 1))
      [ "$attempts" -lt 10 ] || { echo 'macOS could not load the service. Check Login Items permissions and try service start again.' >&2; exit 1; }
      sleep 0.5
    done
  fi
  launchctl kickstart "$target"
  attempts=0
  until responding; do
    attempts=$((attempts + 1))
    [ "$attempts" -lt 20 ] || { echo "Service installed but the dashboard is not ready. Check $logs/stderr.log and run service status." >&2; exit 1; }
    sleep 0.5
  done
  echo "Cuttlefish is running in the background at http://$probe_host:$port."
  echo 'It starts automatically after you log in to this macOS account.'
}
case "$action" in
  install)
    umask 077
    mkdir -p "$agents"
    [ ! -L "$logs" ] || { echo 'Refusing a symlinked service log directory.' >&2; exit 1; }
    mkdir -p "$logs"
    chmod 700 "$logs"
    for name in stdout stderr; do
      [ ! -L "$logs/$name.log" ] || { echo 'Refusing a symlinked service log.' >&2; exit 1; }
      touch "$logs/$name.log"
      chmod 600 "$logs/$name.log"
    done
    temporary=$(mktemp "$agents/.$label.XXXXXX")
    cat > "$temporary" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Cuttlefish login service v1 -->
<!-- Cuttlefish listener: $listen -->
<plist version="1.0"><dict>
<key>Label</key><string>$label</string>
<key>ProgramArguments</key><array>
<string>$(xml "$binary")</string><string>--state-dir</string><string>$(xml "$state")</string>
<string>serve</string><string>--listen</string><string>$(xml "$listen")</string>
</array>
<key>WorkingDirectory</key><string>$(xml "$state")</string>
<key>EnvironmentVariables</key><dict>
<key>HOME</key><string>$(xml "$account_home")</string>
<key>PATH</key><string>$(xml "$PATH")</string>
EOF
    # Preserve provider/config locations only, never copy the entire environment.
    for name in CODEX_HOME CLAUDE_CONFIG_DIR DOCKER_CONFIG XDG_CONFIG_HOME; do
      value=$(printenv "$name" || true)
      [ -z "$value" ] || printf '<key>%s</key><string>%s</string>\n' "$name" "$(xml "$value")" >> "$temporary"
    done
    cat >> "$temporary" <<EOF
</dict>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>AbandonProcessGroup</key><true/>
<key>ThrottleInterval</key><integer>10</integer>
<key>ExitTimeOut</key><integer>20</integer>
<key>LimitLoadToSessionType</key><string>Aqua</string>
<key>StandardOutPath</key><string>$(xml "$logs/stdout.log")</string>
<key>StandardErrorPath</key><string>$(xml "$logs/stderr.log")</string>
</dict></plist>
EOF
    plutil -lint "$temporary" >/dev/null
    if [ ! -f "$plist" ] || ! cmp -s "$temporary" "$plist"; then
      # Check conflicts before creating automatic startup for a busy port.
      if ! loaded && nc -z -G 1 "$host" "$port" >/dev/null 2>&1; then
        echo "Port $port is already in use. Stop the foreground Cuttlefish process, then retry." >&2
        exit 1
      fi
      stop_loaded
      mv "$temporary" "$plist"
      temporary=''
    fi
    start_loaded
    ;;
  start) start_loaded;;
  restart) stop_loaded; start_loaded;;
  stop|uninstall)
    launchctl disable "$target"
    stop_loaded
    if [ "$action" = uninstall ]; then rm "$plist"; fi
    echo 'Cuttlefish automatic startup is disabled. Chats, settings, and provider sign-ins are unchanged.'
    ;;
  status)
    if loaded; then
      if responding; then echo 'Cuttlefish background service is loaded; dashboard is responding.';
      else echo "Cuttlefish background service is loaded; dashboard is not responding. Logs: $logs"; exit 1; fi
    else echo 'Cuttlefish background service is installed but stopped.'; fi
    ;;
esac
