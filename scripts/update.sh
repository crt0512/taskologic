#!/bin/sh
# Upgrade an existing Taskologic install. Called by `make update`.
# Rebuilds, replaces the two binaries and the unit file, then restarts the
# daemon. Config, database, users and groups are left alone: setting those
# up is what `make setup` is for, and an upgrade has no business touching
# them.
#
# Run it as yourself, not with sudo: it asks for root only for the steps that
# need it, so the build reuses your existing target/ instead of recompiling
# the workspace as root.
set -eu

BINDIR="${BINDIR:-/usr/local/bin}"
UNITDIR="${UNITDIR:-/etc/systemd/system}"
SERVICE=taskologicd
# RESTART=no installs the new binaries but leaves the running daemon alone.
RESTART="${RESTART:-yes}"

fail() { echo "error: $1" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "update targets a Linux host with systemd; on other systems follow docs/running.md by hand"

# Elevate the few steps that write outside the tree, nothing else. Building as
# root would use root's CARGO_HOME, rebuild the whole workspace from scratch
# and leave root owned files in target/.
if [ "$(id -u)" = "0" ]; then
    SUDO=""
    if [ -n "${SUDO_USER:-}" ]; then
        echo "note: you ran this under sudo, which is no longer needed. It makes cargo"
        echo "      rebuild everything as root and leaves root owned files in target/."
        echo "      Next time just: make update"
        echo
    fi
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
    echo "This needs root for the install and the service restart, nothing else."
    $SUDO -v || fail "sudo declined"
    echo
else
    fail "not root and no sudo here; re-run as root"
fi

echo "== Checking the install"
missing=0
for f in "$BINDIR/taskologicd" "$BINDIR/taskologic" "$UNITDIR/taskologicd.service"; do
    if [ -e "$f" ]; then
        echo "  found $f"
    else
        echo "  missing $f"
        missing=1
    fi
done
if [ "$missing" = "1" ]; then
    fail "nothing to upgrade here, this host has no install yet. Run 'sudo make setup' instead"
fi

echo
echo "== Building and installing"
make install BINDIR="$BINDIR" UNITDIR="$UNITDIR"

echo
echo "== Daemon"
if [ "$RESTART" != "yes" ]; then
    echo "  RESTART=no, leaving the running daemon on the old binary"
    echo "  apply it later with: sudo systemctl daemon-reload && sudo systemctl restart $SERVICE"
elif systemctl is-enabled --quiet "$SERVICE" 2>/dev/null || systemctl is-active --quiet "$SERVICE" 2>/dev/null; then
    echo "  restarting $SERVICE, which drops any client sessions open right now"
    $SUDO systemctl daemon-reload
    $SUDO systemctl restart "$SERVICE"
    $SUDO systemctl --no-pager --lines 3 status "$SERVICE" || true
else
    echo "  $SERVICE is neither enabled nor running, so there is nothing to restart"
    echo "  start it with: sudo systemctl enable --now $SERVICE"
fi

echo
echo "Done. The daemon is on the new binary."
echo "Clients are not: a session already open keeps the binary it started with,"
echo "so anyone testing a client side change has to log out and back in first."
echo "The daemon logs to the journal: journalctl -u $SERVICE"
