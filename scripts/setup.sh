#!/bin/sh
# First time setup for Taskologic on a Linux host. Called by `make setup`.
# Run it as yourself, not with sudo: it asks for root only where it needs to.
# Checks the install, then asks before every step: create the group and the
# daemon user, enable the service, add the first user. Safe to run again,
# every step skips what already exists.
set -eu

BINDIR="${BINDIR:-/usr/local/bin}"
UNITDIR="${UNITDIR:-/etc/systemd/system}"
GROUP=taskologic
DAEMON_USER=taskologic

fail() { echo "error: $1" >&2; exit 1; }

ask() {
    # ask "question" -> returns 0 for yes
    printf '%s [y/N] ' "$1"
    read -r answer
    case "$answer" in
        y|Y|yes|YES) return 0 ;;
        *) return 1 ;;
    esac
}

[ "$(uname -s)" = "Linux" ] || fail "setup targets a Linux host with systemd; on other systems follow docs/running.md by hand"

# Elevate only the steps that need it. Building as root would use root's
# CARGO_HOME, rebuild the whole workspace and leave root owned files in target/.
if [ "$(id -u)" = "0" ]; then
    SUDO=""
    if [ -n "${SUDO_USER:-}" ]; then
        echo "note: you ran this under sudo, which is no longer needed. It makes cargo"
        echo "      rebuild everything as root and leaves root owned files in target/."
        echo "      Next time just: make setup"
        echo
    fi
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
    echo "This needs root for the install, the accounts and the service, nothing else."
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
    if ask "Not fully installed. Run 'make install' now?"; then
        make install
    else
        fail "install first, then run setup again"
    fi
fi

echo
echo "== Group, daemon user and service"
echo "Taskologic users are the members of the '$GROUP' system group. The daemon"
echo "runs as its own system user and is the only thing that touches the"
echo "database in /var/lib/taskologic."
if ask "Create the group and daemon user if needed, and enable the service?"; then
    if getent group "$GROUP" >/dev/null; then
        echo "  group $GROUP exists"
    else
        $SUDO groupadd --system "$GROUP"
        echo "  created group $GROUP"
    fi
    if id -u "$DAEMON_USER" >/dev/null 2>&1; then
        echo "  user $DAEMON_USER exists"
    else
        $SUDO useradd --system --gid "$GROUP" --home-dir /var/lib/taskologic \
            --shell /usr/sbin/nologin --comment "Taskologic daemon" "$DAEMON_USER"
        echo "  created system user $DAEMON_USER"
    fi
    if [ ! -e /etc/taskologic/taskologicd.toml ]; then
        $SUDO mkdir -p /etc/taskologic
        # Defaults are fine; the file exists so there is something to edit.
        $SUDO tee /etc/taskologic/taskologicd.toml >/dev/null <<'EOF'
# Taskologic daemon config. Missing keys use these defaults.
#socket_path = "/run/taskologic/taskologicd.sock"
#db_path = "/var/lib/taskologic/taskologic.db"
#group = "taskologic"
#always_admin_uids = []
#host_timezone = "Europe/Berlin"
EOF
        echo "  wrote /etc/taskologic/taskologicd.toml"
    fi
    $SUDO systemctl daemon-reload
    $SUDO systemctl enable --now taskologicd
    echo "  service enabled and started"
    $SUDO systemctl --no-pager --lines 3 status taskologicd || true
else
    echo "  skipped"
fi

echo
echo "== First user"
echo "Members of '$GROUP' can use Taskologic. The first user to connect becomes"
echo "an admin and can promote others from inside the client."
if ask "Add a user to the $GROUP group now?"; then
    printf 'Username: '
    read -r username
    id -u "$username" >/dev/null 2>&1 || fail "no such user: $username"
    $SUDO usermod -aG "$GROUP" "$username"
    echo "  added $username to $GROUP (takes effect on their next login)"
    echo "  they can now run: taskologic"
    echo "  to make it their login shell: add $BINDIR/taskologic to /etc/shells,"
    echo "  then: sudo chsh -s $BINDIR/taskologic $username"
else
    echo "  skipped"
fi

echo
echo "Done. The daemon logs to the journal: journalctl -u taskologicd"
