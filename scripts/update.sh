#!/bin/sh
# Upgrade an existing Taskologic install. Called by `make update`.
#
# Reads the version that is installed right now, rebuilds, replaces the two
# binaries and the unit file, brings the database up to the schema the new
# build expects, then restarts the daemon. Config, users and groups are left
# alone: setting those up is what `make setup` is for, and an upgrade has no
# business touching them.
#
# The database is the one thing an upgrade cannot leave alone, because a new
# build can expect columns the old schema has not got. The daemon migrates on
# start anyway, but doing it here, deliberately and with a copy taken first,
# means a bad upgrade has something to go back to and the operator can see
# which version they came from.
#
# Run it as yourself, not with sudo: it asks for root only for the steps that
# need it, so the build reuses your existing target/ instead of recompiling
# the workspace as root.
set -eu

BINDIR="${BINDIR:-/usr/local/bin}"
UNITDIR="${UNITDIR:-/etc/systemd/system}"
SERVICE=taskologicd
# RESTART=no installs the new binaries but leaves the running daemon alone.
# The database is left alone too: migrating out from under a daemon that is
# still running the old binary is exactly the mess this script exists to avoid.
RESTART="${RESTART:-yes}"

fail() { echo "error: $1" >&2; exit 1; }

# The version a binary reports. Builds before 0.1.10 had no --version to ask,
# so they answer with a usage error and get reported as unknown.
binary_version() {
    v=""
    if [ -x "$1" ]; then
        v="$("$1" --version 2>/dev/null)" || v=""
    fi
    case "$v" in
        '') echo "unknown (built before 0.1.10)" ;;
        *) echo "${v##* }" ;;
    esac
}

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
    echo "This needs root for the install, the database migration and the service"
    echo "restart, nothing else."
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

# Ask the installed binary what it is before overwriting it; afterwards there
# is nothing left to ask.
FROM_VERSION="$(binary_version "$BINDIR/taskologicd")"
echo "  installed version: $FROM_VERSION"

# Whether to put the daemon back the way it was found.
if systemctl is-active --quiet "$SERVICE" 2>/dev/null; then
    WAS_ACTIVE=yes
else
    WAS_ACTIVE=no
fi
if systemctl is-enabled --quiet "$SERVICE" 2>/dev/null; then
    WAS_ENABLED=yes
else
    WAS_ENABLED=no
fi

echo
echo "== Building and installing"
make install BINDIR="$BINDIR" UNITDIR="$UNITDIR"
TO_VERSION="$(binary_version "$BINDIR/taskologicd")"
echo "  $FROM_VERSION -> $TO_VERSION"

echo
echo "== Database"
if [ "$RESTART" != "yes" ]; then
    echo "  RESTART=no, so the old daemon is still running against the old schema"
    echo "  and the database has been left exactly as it was."
    echo "  Migrate and restart together when you are ready:"
    echo "    sudo systemctl stop $SERVICE"
    echo "    sudo $BINDIR/taskologicd --migrate"
    echo "    sudo systemctl daemon-reload && sudo systemctl start $SERVICE"
else
    if [ "$WAS_ACTIVE" = "yes" ]; then
        echo "  stopping $SERVICE so nothing holds the database open"
        echo "  this drops any client sessions open right now"
        $SUDO systemctl stop "$SERVICE"
    fi
    # --migrate takes its own copy first and reports what it did. It reads the
    # database path out of the daemon config, so this script never has to know
    # where the file lives. Deliberately not piped through anything: a pipeline
    # would report the exit status of the last command in it, and a failed
    # migration has to stop us before the daemon goes back up on a schema it
    # cannot use.
    $SUDO "$BINDIR/taskologicd" --migrate || fail \
        "migration failed, so $SERVICE has NOT been restarted and is still down.
       The copy taken before the migration is beside the database as
       taskologic.db.bak-*. Put the old binaries back and restore that copy if
       you need the host working again right now."
fi

echo
echo "== Daemon"
if [ "$RESTART" != "yes" ]; then
    echo "  RESTART=no, leaving the running daemon on the old binary"
elif [ "$WAS_ACTIVE" = "yes" ] || [ "$WAS_ENABLED" = "yes" ]; then
    echo "  starting $SERVICE on the new binary"
    $SUDO systemctl daemon-reload
    $SUDO systemctl restart "$SERVICE"
    $SUDO systemctl --no-pager --lines 3 status "$SERVICE" || true
else
    echo "  $SERVICE is neither enabled nor running, so there is nothing to start"
    echo "  start it with: sudo systemctl enable --now $SERVICE"
fi

echo
echo "Done. The daemon is on $TO_VERSION."
echo "Clients are not: a session already open keeps the binary it started with,"
echo "so anyone testing a client side change has to log out and back in first."
echo "The database was copied beside itself before migrating; once the new"
echo "version has proven itself those .bak- files are yours to delete."
echo "The daemon logs to the journal: journalctl -u $SERVICE"
