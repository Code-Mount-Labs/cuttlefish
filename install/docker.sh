#!/bin/sh
# Install only the database dependency selected by the local administrator.
# Existing Docker installations and container data are never removed.
set -eu
mode=${1:---non-interactive}
root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif [ "$mode" = --interactive ]; then sudo -- "$@"
  else sudo -n -- "$@"
  fi
}
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then exit 0; fi
case "$(uname -s)" in
  Linux)
    if command -v docker >/dev/null 2>&1; then
      if [ "$(id -u)" -eq 0 ]; then systemctl start docker
      else systemctl --user start docker || root systemctl start docker
      fi
    else
      # Only the official distributions whose repository layout is known here.
      . /etc/os-release
      case "$ID" in ubuntu|debian) ;; *) echo 'Automatic Docker installation supports Ubuntu and Debian. Install Docker Engine for this distribution, then retry.' >&2; exit 78;; esac
      codename=${UBUNTU_CODENAME:-${VERSION_CODENAME:-}}
      case "$codename" in ''|*[!a-z0-9]*) echo 'Unsupported distribution codename.' >&2; exit 78;; esac
      root apt-get update
      root apt-get install -y --no-remove ca-certificates curl uidmap dbus-user-session
      root install -m 0755 -d /etc/apt/keyrings
      temporary=$(mktemp -d)
      trap 'rm -rf "$temporary"' EXIT HUP INT TERM
      curl --proto '=https' --tlsv1.2 -fsSL "https://download.docker.com/linux/$ID/gpg" -o "$temporary/docker.asc"
      cat > "$temporary/docker.sources" <<SOURCES
Types: deb
URIs: https://download.docker.com/linux/$ID
Suites: $codename
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
SOURCES
      if [ -e /etc/apt/sources.list.d/docker.sources ] && ! cmp -s /etc/apt/sources.list.d/docker.sources "$temporary/docker.sources"; then
        echo 'An existing Docker repository configuration differs; review it before retrying.' >&2; exit 78
      fi
      root install -m 0644 "$temporary/docker.asc" /etc/apt/keyrings/docker.asc
      root install -m 0644 "$temporary/docker.sources" /etc/apt/sources.list.d/docker.sources
      root apt-get update
      root apt-get install -y --no-remove docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin docker-ce-rootless-extras
      if [ "$(id -u)" -eq 0 ]; then
        systemctl enable --now docker
      else
        # Do not grant the application account root-equivalent Docker group access.
        dockerd-rootless-setuptool.sh install --force
        root loginctl enable-linger "$(id -un)"
      fi
    fi
    docker info >/dev/null
    ;;
  Darwin)
    if [ ! -d /Applications/Docker.app ]; then
      case "$(uname -m)" in arm64) arch=arm64;; x86_64) arch=amd64;; *) exit 78;; esac
      temporary=$(mktemp -d)
      mounted=no
      cleanup() { if [ "$mounted" = yes ]; then hdiutil detach "$temporary/mount" >/dev/null || true; fi; rm -rf "$temporary"; }
      trap cleanup EXIT HUP INT TERM
      mkdir "$temporary/mount"
      curl --proto '=https' --tlsv1.2 -fsSL "https://desktop.docker.com/mac/main/$arch/Docker.dmg" -o "$temporary/Docker.dmg"
      hdiutil attach -readonly -nobrowse -mountpoint "$temporary/mount" "$temporary/Docker.dmg" >/dev/null
      mounted=yes
      codesign --verify --deep --strict "$temporary/mount/Docker.app"
      spctl --assess --type execute "$temporary/mount/Docker.app"
      root "$temporary/mount/Docker.app/Contents/MacOS/install" "--user=$(id -un)"
    fi
    open -a Docker
    # Docker Desktop may ask the user to complete its own first-run setup.
    count=0
    while [ "$count" -lt 60 ]; do
      if /Applications/Docker.app/Contents/Resources/bin/docker info >/dev/null 2>&1; then exit 0; fi
      count=$((count+1)); sleep 2
    done
    echo 'Complete Docker Desktop setup, then retry the saved database setup.' >&2; exit 78
    ;;
  *) echo 'Use the Windows installer or install Docker for this operating system.' >&2; exit 78;;
esac
