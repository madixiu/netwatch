#!/usr/bin/env bash
# NetWatch one-command deploy.
# Builds on this PC, copies to the server, installs, restarts, and checks health.
#
# Usage:
#   ./deploy.sh            full deploy: backend + frontend + restart
#   ./deploy.sh --web      frontend only: no restart, no sudo, live at once
#   ./deploy.sh --check    build only, nothing is sent to the server
#
# Settings (override with environment variables if needed):
#   NW_SRC     project folder on this PC   (default: ~/Desktop/netwatch)
#   NW_SERVER  ssh target                  (default: pak@172.16.66.6)
#   NW_STAGE   folder in the server home   (default: Desktop)
#
# Requirements on this PC: cargo, npm, rsync, ssh.
# Requirements on the server: rsync, sudo.

set -euo pipefail

NW_SRC="${NW_SRC:-$HOME/Desktop/netwatch}"
NW_SERVER="${NW_SERVER:-pak@172.16.66.6}"
NW_STAGE="${NW_STAGE:-Desktop}"
MODE="${1:-full}"

case "$MODE" in
  full|--web|--check) ;;
  *) echo "Usage: $0 [--web|--check]"; exit 2 ;;
esac

say() { printf '\n==> %s\n' "$*"; }
die() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

cd "$NW_SRC" 2>/dev/null || die "project not found at $NW_SRC (set NW_SRC)"
[ -f Cargo.toml ] || die "$NW_SRC is not the netwatch project"
for tool in rsync ssh; do
  command -v "$tool" >/dev/null || die "$tool is not installed on this PC"
done

# One SSH connection is shared by every command below, so the password is asked once.
mkdir -p "$HOME/.ssh"
SSH_OPTS=(-o ControlMaster=auto -o ControlPersist=15m -o "ControlPath=$HOME/.ssh/nw-%C")
RSYNC_RSH="ssh ${SSH_OPTS[*]}"

build_frontend() {
  say "Building frontend"
  [ -d web/node_modules ] || npm --prefix web ci --no-audit --no-fund
  npm --prefix web run build
  [ -f web/dist/index.html ] || die "frontend build produced no index.html"
}

build_backend() {
  command -v cargo >/dev/null || die "cargo is not installed on this PC"
  say "Building backend (release)"
  cargo build --release -p netwatch-poller -p netwatch-api --bins

  # Refuse to deploy a build that does not contain the latest code.
  say "Checking the build contains the new code"
  grep -q 'if_vlan' target/release/netwatch-poller \
    || die "netwatch-poller has no VLAN code. Run apply_backend.py on this project first."
  grep -q 'ip_forwarding' target/release/netwatch-poller \
    || die "netwatch-poller has no layer 2/3 code. Run apply_backend.py on this project first."
  grep -q 'api/macs' target/release/netwatch-api \
    || die "netwatch-api has no MAC endpoint. Run apply_backend.py on this project first."
  echo "build contains the new code"
}

push_frontend() {
  say "Sending frontend to $NW_SERVER:$NW_STAGE/dist"
  rsync -az --delete -e "$RSYNC_RSH" web/dist/ "$NW_SERVER:$NW_STAGE/dist/"
}

if [ "$MODE" = "--web" ]; then
  build_frontend
  push_frontend
  say "Frontend updated. Reload the browser (Ctrl+Shift+R). No restart needed."
  exit 0
fi

build_backend
build_frontend

if [ "$MODE" = "--check" ]; then
  say "Build OK. Nothing was sent to the server."
  exit 0
fi

# The install steps run on the server from a file, so sudo gets a real terminal.
INSTALL_SCRIPT="$(mktemp)"
trap 'rm -f "$INSTALL_SCRIPT"' EXIT
cat > "$INSTALL_SCRIPT" <<'REMOTE'
#!/usr/bin/env bash
set -euo pipefail

STAGE="$HOME/${NW_STAGE:-Desktop}"
WEB="$STAGE/dist"
ENV=/etc/netwatch/netwatch.env

echo "--- stopping services"
sudo systemctl stop netwatch-api netwatch-poller 2>/dev/null || true

echo "--- installing programs"
sudo install -m 0755 "$STAGE/netwatch-poller" "$STAGE/netwatch-api" \
  "$STAGE/netwatch-migrate" "$STAGE/netwatch-user" /opt/netwatch/bin/

echo "--- installing service files"
sudo install -m 0644 "$STAGE/deploy/systemd/netwatch-poller.service" \
  "$STAGE/deploy/systemd/netwatch-api.service" /etc/systemd/system/
sudo systemctl daemon-reload

echo "--- web root: served from $WEB"
# Only read and traverse permissions are added. Nothing becomes writable.
chmod o+x "$HOME" "$HOME/${NW_STAGE:-Desktop}" 2>/dev/null || true
chmod -R a+rX "$WEB"
sudo sed -i '/^NETWATCH_WEB_ROOT=/d' "$ENV"
echo "NETWATCH_WEB_ROOT=$WEB" | sudo tee -a "$ENV" >/dev/null

echo "--- migrations"
sudo bash -c 'set -a; . /etc/netwatch/netwatch.env; set +a; /opt/netwatch/bin/netwatch-migrate'

echo "--- starting services"
sudo systemctl enable netwatch-poller netwatch-api >/dev/null
sudo systemctl start netwatch-poller netwatch-api
sleep 3
systemctl is-active netwatch-poller netwatch-api || true

if curl -fsS http://127.0.0.1:8080/healthz >/dev/null; then
  echo "API health: OK"
else
  echo "API health: FAILED. Run: sudo journalctl -u netwatch-api -n 30 --no-pager"
fi
REMOTE

say "Sending programs and deploy files to $NW_SERVER:$NW_STAGE"
rsync -az -e "$RSYNC_RSH" \
  target/release/netwatch-poller target/release/netwatch-api \
  target/release/netwatch-migrate target/release/netwatch-user \
  "$NW_SERVER:$NW_STAGE/"
rsync -az --delete -e "$RSYNC_RSH" deploy/ "$NW_SERVER:$NW_STAGE/deploy/"
rsync -az -e "$RSYNC_RSH" "$INSTALL_SCRIPT" "$NW_SERVER:$NW_STAGE/install-netwatch.sh"
push_frontend

say "Installing on the server (enter the server sudo password when asked)"
ssh -t "${SSH_OPTS[@]}" "$NW_SERVER" "NW_STAGE='$NW_STAGE' bash \$HOME/$NW_STAGE/install-netwatch.sh"

say "Deploy finished. Open the dashboard and press Ctrl+Shift+R."
