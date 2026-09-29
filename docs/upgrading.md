# Upgrading an install

How a host that went through `make setup` moves to a new version, what happens to the database, and what each version changed on disk.

`make update` fetches and pulls the latest source; `make upgrade` is then
the whole upgrade: it rebuilds, installs both binaries and
the unit, stops the daemon, backs the database up beside itself, applies
whatever migrations are pending and starts the daemon again. `make db-status`
says where a database stands without touching it.

0.1.12 adds migrations 0009 to 0011 (print rules on tasks, programs and
their runs, the auto start flag). They add columns with defaults and empty
tables, so an install comes forward with nothing to do by hand. The protocol
went to 3 at the same time: a 0.1.11 client is refused by a 0.1.12 daemon
rather than allowed to strip the new fields off every task it saves. Anyone
with a session open has to log out and back in to get the new client.

0.1.13 adds migration 0012, one column with a default: how many finished
tasks a program wants before the board shows an estimate. Programs saved by
0.1.12 keep loading, a "next day at" start rule included, which reads as one
day. The protocol went to 4, since a 0.1.12 client cannot read the new start
rule; the same log out and back in applies.

0.2.0 (barcode controls) adds migration 0013: a short id column on boards,
templates and programs, filled in by the daemon the first time it opens the
database, so nothing on disk needs a hand. The `-- dashes` choice for task
codes is gone from Settings; task codes are `..`. The protocol went to 5,
since an older client cannot read a board that carries a short id; the same
log out and back in applies.

0.2.1 - various bug fixes

0.2.2 - further bug fixes, addition of further default themes that now look
more sexy in my opionion, better touch controlls here and there.

## What `make upgrade` does

Once a host has been through `make setup`, new versions go on with:

```bash
make update    # git fetch and git pull
make upgrade   # build, install, migrate, restart
```

Run it as yourself, **not** with `sudo`. All of these targets ask for root
only for the handful of steps that write outside the tree, and build as you.
Putting `sudo` in front instead builds as root, which uses root's `CARGO_HOME`,
so every dependency fingerprint changes, the whole workspace recompiles from
scratch, and `target/` fills with root owned files your next ordinary build
cannot overwrite.

That rebuilds, replaces both binaries and the unit file, brings the database
up to the schema the new build expects, reloads systemd and restarts the
daemon. It never touches `/etc/taskologic`, the group or the users. If the
host has no install yet it stops and tells you to run `make setup` instead,
rather than half installing something.

It prints which version it found installed and which one it is putting on, so
you can see what you came from:

```
== Checking the install
  found /usr/local/bin/taskologicd
  ...
  installed version: 0.1.9
== Building and installing
  0.1.9 -> 0.1.10
== Database
  stopping taskologicd so nothing holds the database open
  backed up to /var/lib/taskologic/taskologic.db.bak-20260913-140301
  migrated /var/lib/taskologic/taskologic.db from schema version 6 to 7
```

Versions before 0.1.10 had no `--version` to ask, so upgrading off one of
those reports the installed version as unknown. Every upgrade from 0.1.10
onwards knows.

### About the database

New versions sometimes add columns. The daemon has always migrated on start,
so this is not new; what `make upgrade` adds is doing it **deliberately**, with
the daemon stopped and a copy taken first, instead of as a side effect of the
next restart. The copy lands beside the database as
`taskologic.db.bak-<date>-<time>`, owned by the same account as the database,
and nothing ever deletes it. Once the new version has proven itself those
files are yours to remove.

Migrations only run forwards. If you put an **older** build back on a database
a newer one has already migrated, `--migrate` refuses and says so rather than
guessing; restore the matching `.bak-` copy.

You can look without changing anything, while the daemon is running:

```bash
make db-status
```

To migrate by hand, which you should not normally need:

```bash
sudo systemctl stop taskologicd
make migrate
sudo systemctl start taskologicd
```

The restart drops client sessions that are open at that moment. To install
now and restart later:

```bash
make upgrade RESTART=no
```

`RESTART=no` leaves the database alone as well, because migrating out from
under a daemon that is still running the old binary is the one thing worth
avoiding here. Finish the job when it suits:

```bash
sudo systemctl stop taskologicd
sudo /usr/local/bin/taskologicd --migrate
sudo systemctl daemon-reload && sudo systemctl start taskologicd
```

One thing that catches people out: **a running client keeps the binary it
started with**. The daemon is on the new version the moment it restarts, but
anyone testing a client side change (the printer panel, the UI, anything in
the TUI) has to exit their session and log in again to actually be running
it.
