# Taskologic tingles the TISM how to deploy ?!

Theres two binaries: `taskologicd` (demon, master of the db) and `taskologic` (tui
client). Both read TOML config files. (They are betraying JSON Derulo)

I recommend using the `make setup` in projectroot for setting this up somewhat more easily. Should guide you through it all no pain or struggle (if you have rustup toolchain installed)

For documentation purposes though heres how one would set it up by hand :

## Build and check

Bare `make` builds release binaries, `make help` lists the rest.

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

Note : If you are deving stuff and make changes to the ui and now have all the ui tests failing make sure to update the UI snapshots, run `INSTA_UPDATE=always cargo test -p taskologic` and review the diff in `crates/taskologic/src/ui/snapshots/` to make sure its all happy.

## Deploying

On a Linux host with systemd, `make setup` does the whole first install: the `taskologic` group, the daemon's system user, the unit, the first login. `make update` upgrades it later; that, the database backup it takes and what each version changed are in [upgrading.md](upgrading.md). By hand, the pieces are the two configs below, a group your users are in, and the daemon as a service that owns the database and the socket directory.

### Installing as a login shell

Add the `taskologic` binary path to `/etc/shells`, then set it as your taskologic users shell in `/etc/passwd`, and put the user in the `taskologic` group. The daemon runs as its own system user that owns the database file and the socket directory. Nobody else needs access to the database.

If you need telnet, enable that on ur servers linux distro (usefull for when your old Windows CE tablet cant do modern ssh)

### Daemon config

The default path is here : `/etc/taskologic/taskologicd.toml`, override with `--config PATH` or `TASKOLOGICD_CONFIG`. 
If its missing its gonna use default options.
Did it this way for ease of development, in ur config make sure to point the socket and database somewhere writable and use a group you are in:

```toml
socket_path = "/tmp/taskologicd.sock"
db_path = "/tmp/taskologic-dev.db"
group = "staff" # ooo mac os ah group ik, in prod use "taskologic" or smth
always_admin_uids = []
print_job_max_age_secs = 14400
scheduler_tick_secs = 30
# host_timezone = "Europe/Zurich"
```

for debug run with `TASKOLOGICD_LOG=debug cargo run -p taskologicd -- --config dev.toml`.
The first user to connect becomes an admin automatically. Ik very safety

### Client config

First file that exists wins: `$TASKOLOGIC_CONFIG`, `~/.config/taskologic/client.toml`,
`/etc/taskologic/client.toml`.

```toml
socket_path = "/tmp/taskologicd.sock"
print_priority = 0           # higher wins when you have several clients for same user open
# ascii = true               # force ASCII borders

# [printer] goes here on a client with a receipt printer; see printers.md.
```

The printer and scanner settings live with the user and the machine on purpose, not server wide: the person at a terminal knows what is plugged into it.

## Printers, tldr

Plug the receipt printer in and set it up as a CUPS queue the way you would any printer. In the client, Settings (`S`) -> Printer lists the queues, picks the paper width from what the queue reports, and prints a test slip. The default output mode, bitmap, prints on anything CUPS can drive; ESC/POS is faster on a printer that speaks it. If the test slip does not come out, or comes out sideways, blank or without barcodes, the answers are in [printers.md](printers.md), which also covers what a receipt slip shows and in what order.

## Barcodes, tldr

Any scanner that types like a keyboard and presses enter after that will most likeyly work.
Turn the scanner on in Settings, tick "presses Enter" if yours does, and scan a receipt slip's code: the task starts, pauses or finishes. [barcodes.md](barcodes.md) has the settings, what each code on a receipt slip does and how a scan is told apart from typing. 

Beyond task slips there are codes that drive the client itself, keys and screens and more, printed from Settings -> Print codes, those are in [barcode-commands.md](barcode-commands.md).

## Moving data between boards and servers

`scripts/transfer.sh` exports and imports programs, templates, boards and whole servers as JSONDerulo, with a way to remap users between servers. See [transfer.md](transfer.md).

## Further reading

[README.md](README.md) in this folder lists every page.
