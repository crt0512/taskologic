#!/bin/sh
# Move things between boards or servers as JSON files. Called by hand:
#
#   scripts/transfer.sh export program  "Kitchen" "Clean up"  cleanup.json
#   scripts/transfer.sh import program  "Garage"  cleanup.json
#   scripts/transfer.sh export template "Kitchen" "Wash up"   washup.json
#   scripts/transfer.sh import template "Garage"  washup.json
#   scripts/transfer.sh export board    "Kitchen" kitchen.json      (or: all)
#   scripts/transfer.sh import board    kitchen.json
#   scripts/transfer.sh export all      server.json
#   scripts/transfer.sh import all      server.json
#   scripts/transfer.sh users
#   scripts/transfer.sh users remap     server.json [remapped.json]
#
# An import may end in --new-id or --replace: what to do when this server
# already has a board, template or program with the file's short id (the
# six characters barcodes name it by). Without either, it refuses and says
# which board has it. --replace overwrites a template or program in place.
# Without a file an export prints to stdout; an import reads "-" as stdin.

set -eu

fail() { echo "error: $1" >&2; exit 1; }

usage() {
    sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

[ $# -ge 1 ] || usage
verb=$1
shift
outfile=""
case "$verb" in
    users)
        if [ $# -eq 0 ]; then
            set -- --users
        elif [ "$1" = remap ] && { [ $# -eq 2 ] || [ $# -eq 3 ]; }; then
            shift
            outfile="${2:-$1}"
            set -- --remap-users "$@"
        else
            usage
        fi
        ;;
    export|import)
        [ $# -ge 1 ] || usage
        kind=$1
        shift
        # --new-id or --replace anywhere among an import's arguments, handed
        # to the daemon as --on-clash.
        clash=""
        if [ "$verb" = import ]; then
            n=$#
            i=0
            while [ $i -lt $n ]; do
                arg=$1
                shift
                i=$((i + 1))
                case "$arg" in
                    --new-id) clash="new" ;;
                    --replace) clash="replace" ;;
                    *) set -- "$@" "$arg" ;;
                esac
            done
        fi
        case "$verb/$kind" in
            export/program|export/template) { [ $# -eq 2 ] || [ $# -eq 3 ]; } || usage; outfile="${3:-}" ;;
            import/program|import/template) [ $# -eq 2 ] || usage ;;
            export/board) { [ $# -eq 1 ] || [ $# -eq 2 ]; } || usage; outfile="${2:-}" ;;
            export/all) [ $# -le 1 ] || usage; outfile="${1:-}" ;;
            import/board|import/all) [ $# -eq 1 ] || usage ;;
            *) usage ;;
        esac
        set -- "--$verb-$kind" "$@"
        if [ -n "$clash" ]; then
            set -- "$@" --on-clash "$clash"
        fi
        ;;
    *) usage ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Which binary: TASKOLOGICD if set, the installed daemon, or this checkout's
# own build, release before debug.
BIN="${TASKOLOGICD:-}"
if [ -z "$BIN" ]; then
    for candidate in "${BINDIR:-/usr/local/bin}/taskologicd" \
                     "$ROOT/target/release/taskologicd" \
                     "$ROOT/target/debug/taskologicd"; do
        if [ -x "$candidate" ]; then
            BIN="$candidate"
            break
        fi
    done
fi
[ -n "$BIN" ] || fail "no taskologicd found; set TASKOLOGICD, or build or install first"

# Which config: TASKOLOGICD_CONFIG if set, the dev checkout's, else the
# daemon's default path, which taskologicd knows itself.
CFG="${TASKOLOGICD_CONFIG:-}"
if [ -z "$CFG" ] && [ -f "$ROOT/dev/taskologicd.toml" ]; then
    CFG="$ROOT/dev/taskologicd.toml"
fi
if [ -n "$CFG" ]; then
    set -- --config "$CFG" "$@"
fi

# Root only when the database is not ours to write, the way make update does.
# taskologicd still checks who asked: sudo passes that through as SUDO_UID.
DB=""
if [ -n "$CFG" ] && [ -r "$CFG" ]; then
    DB="$(sed -n 's/^db_path *= *"\([^"]*\)".*/\1/p' "$CFG" | head -n 1)"
elif [ -r /etc/taskologic/taskologicd.toml ]; then
    DB="$(sed -n 's/^db_path *= *"\([^"]*\)".*/\1/p' /etc/taskologic/taskologicd.toml | head -n 1)"
fi
DB="${DB:-/var/lib/taskologic/taskologic.db}"
SUDO=""
if [ "$(id -u)" != 0 ] && [ -e "$DB" ] && [ ! -w "$DB" ]; then
    command -v sudo >/dev/null 2>&1 || fail "the database $DB is not writable and there is no sudo here; run as root"
    SUDO=sudo
fi

$SUDO "$BIN" "$@"

# A file written by root is handed back to whoever asked for it.
if [ -n "$outfile" ] && [ "$outfile" != - ] && [ -n "$SUDO" ] && [ -e "$outfile" ]; then
    $SUDO chown "$(id -u):$(id -g)" "$outfile"
fi
