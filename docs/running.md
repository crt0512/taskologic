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

## Demon config

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

## Client config

First file that exists wins: `$TASKOLOGIC_CONFIG`, `~/.config/taskologic/client.toml`,
`/etc/taskologic/client.toml`.

```toml
socket_path = "/tmp/taskologicd.sock"
print_priority = 0           # higher wins when you have several clients for same user open
# ascii = true               # force ASCII borders

# Leave this section out on a client/server without a printer.
[printer]
queue = "receipt"            # CUPS queue name
output = "bitmap"            # bitmap (default), esc_pos, text
# What a slip shows is written under [[printer.slips.task]] and
# [[printer.slips.reminder]], one table a section, in print order. Easier set
# in the panel than by hand; a config that names only some sections gains the
# rest with their defaults rather than losing them off the slip.
rotation = "upright"         # upright, cw180, cw90, cw270 (bitmap only)
paper = "mm80"               # "mm58", "mm80", or "custom:72" = printable mm
codepage = "utf8"            # utf8, cp437, cp850, cp858, wpc1252 (esc_pos only)
auto_cutter = true
raw = false                  # true adds `lp -o raw` (esc_pos only)
native_symbologies = ["code39", "code128"]
```

You do not have to write any of this by hand, the client has a setup panel:
Settings (`S`) -> Printer. It lists the queues `lpstat -e` reports, and
everything except the queue and the cutter lives behind Advanced, because
most printers do not care.

Picking a queue also asks CUPS what media that queue is set up for
(`lpoptions -p QUEUE`) and sets the paper width to match, so you do not have
to measure a roll. A Star TSP143 reporting
`media=custom_71.97x199.67mm_...` lands on the 80 mm preset, which prints
72 mm wide. Receipt queues get named after the roll (58/80 mm) or after the
printable area (48/72 mm) depending on who set them up; both spellings of
the same roll land on the same preset. Change the width by hand and your
choice sticks: the panel stops overriding it and just tells you what the
queue says.

### Print as: what the printer is actually fed

`output` is the setting that decides whether anything comes out at all.

| mode | what goes down the wire | barcodes | needs |
|---|---|---|---|
| `esc_pos` | ESC/POS commands | printer's own, or drawn | a printer that speaks ESC/POS |
| `bitmap` | the slip drawn as a 1 bit PBM | drawn here | any queue CUPS can drive |
| `text` | plain text | **none** | any queue CUPS can drive |

**esc_pos** is the fast one. The printer sets the type with its own fonts,
draws its own barcodes and works its own cutter, and the job is about a
kilobyte. Pick it when you know the printer speaks ESC/POS.

**bitmap** is the default, because it is the mode most likely to put ink on
paper on a printer nobody has told the client anything about yet. It draws
the whole slip here, at the paper's own dot width, and hands
CUPS a picture for the queue's driver to rasterise. Use it when the printer
has no fonts of its own. A **Star TSP100 / TSP143 is exactly this case**: it
holds no font sets at all, so ESC/POS text sent to one is simply discarded,
the job completes, and nothing comes out. Bitmap is the mode that prints on
those, and unlike text it keeps the barcodes. The trade is size and speed:
the job is tens of kilobytes and the printer has to pull it all down.

**text** hands CUPS plain text and lets the driver set it. It is the simplest
thing that can work on a driver queue, and it cannot draw barcodes: bars
cannot be made out of characters at a density any scanner would read. The
payload is printed as text instead so a person can still read it.

`rotation` turns the finished bitmap, and only bitmap output can do it: the
other two hand their work to something else to place. `cw180` is the one
receipt printers actually want, for a roll that feeds the other way round. A
quarter turn swaps the slip's width and height, so it only fits if the slip
is shorter than the roll is wide; past that the driver shrinks or clips it,
and the panel says so when you pick one.

Rotation is ours alone: bitmap jobs go out with `orientation-requested=3`,
which pins them to portrait. Left to itself the CUPS image filter turns a
picture sideways whenever landscape would fit the page better, and on a
receipt roll that means a slip a millimetre too wide prints *along* the
paper. If yours ever comes out sideways with `rotation = "upright"`, that
filter is the first place to look, not this setting.

A drawn slip is also kept a millimetre narrower than the paper it is named
after. The widths here are round numbers but a queue's media is whatever
someone measured: an 80 mm roll is set up as 71.97 mm, while 576 dots at
203 dpi is 72.07 mm, which is wider than the page. That tenth of a
millimetre is enough to make the filter rotate. The slack costs one
character a line and takes the whole problem away.

The panel greys out what a mode does not use. Character set and raw are
ESC/POS notions, so in the other two modes those rows explain themselves
rather than offering a control that does nothing, and `raw` is ignored
outright for a bitmap or text job, which exists to be filtered.

### Raw, and why an ESC/POS printer might print nothing

For `esc_pos`, `raw` decides how the job is handed to CUPS:

- **off** (default): plain `lp -d queue`. CUPS types the job and runs it
  through the queue's filters, which is what a queue set up with a driver
  expects.
- **on**: `lp -d queue -o raw`. The ESC/POS bytes reach the printer
  untouched. This is what a socket, serial or parallel queue usually wants,
  and it is the only thing that works if the queue has no driver at all.

Worth knowing: a CUPS test page or a LibreOffice document printing fine only
proves the *filtered* path works, it says nothing about the raw one. Raw is
also deprecated in current CUPS and gone in 2.5, and a driverless/IPP queue
will take a raw job and then quietly abort it. If Test print reports success
and no paper moves, that is the shape of it, check with:

```bash
lpstat -v receipt          # ipp:// or dnssd:// means driverless, raw will not work
lpstat -o                  # is the job sitting there, or was it aborted
tail -30 /var/log/cups/error_log
```

### What a slip shows

Settings (`S`) -> Printer -> **What it prints**. One row a section, in the
order they print, and nothing outside this panel has a say: the daemon sends
everything a task has and this decides what reaches the paper.

There are two of these, one for **receipts** and one for **reminders**, and
the button under the grid swaps between them. They are kept apart rather than
folded into one list with a "which kind of slip" column, because the two want
different *orders*, not only different sections: a reminder can lead with what
is due where a receipt leads with what to do, and one shared order cannot say
both. The cost is that a change you want on both gets made twice.

```
                   Show      Bold      Large     Centered
Custom line        [ ]       [ ]       [ ]       [x]
Board title        [x]       [x]       [ ]       [x]
Task title         [x]       [x]       [x]       [ ]
Barcode: start     [ ]       [ ]       [ ]       [x]     (on for reminders)
Description        [x]       [ ]       [ ]       [ ]
Checklist          [x]       [ ]       [ ]       [ ]
Start date         [ ]       [ ]       [ ]       [x]     (on for reminders)
Due date           [x]       [ ]       [x]       [x]
Creator            [ ]       [ ]       [ ]       [ ]
Assignees          [ ]       [ ]       [ ]       [ ]
Dependencies       [x]       [ ]       [x]       [x]
Barcode: finish    [x]       [ ]       [ ]       [x]     (off for reminders)
Date and time      [x]       [x]       [ ]       [x]
```

Arrows walk the grid, Space works the cell under the cursor, and **Shift with
an arrow carries a whole row up or down the slip**. With a mouse or a finger:
clicking anywhere in a column works that box, and the `^` and `v` at the end
of each row move it, so the order can be set without touching the keyboard.
A row at the end of the list has no button in the direction it cannot go.

The barcodes and the start date are the rows whose defaults differ. Nothing
has been started when a reminder prints, so it carries the code that starts
the task and says when that is due to happen; a receipt is handed over as the
work begins, so it carries the code that finishes it and does not need telling
when to start.

The custom line is your own words, a shop name or a machine number; it is off
by default and the field at the bottom sets what it says. The task's short id
rides along under the task title, since that is what someone reads back to
find the task again.

The layout lives in the printer profile, so it is per machine, and a slip
that prints wrong is one panel's fault rather than a hunt through
preferences. The old print switches in the settings screen are gone.

The test print always carries a sample barcode with `123456789` in it,
whatever the scanner prefs say, so there is something to hold a scanner up
to. Text output cannot draw one and prints the payload as text instead,
which still tells you the slip got that far.

One caveat on the success message: `lp` returns as soon as the job is
*queued*, so "test print sent to the printer" means CUPS accepted it, not
that anything was printed.

Run it with `cargo run -p taskologic`.

## Talking to the DEMONS in your head.

The protocol is in beloved JSON DERULO! so using socat is enough:

```
socat - UNIX-CONNECT:/tmp/taskologicd.sock
{"id":1,"request":{"type":"hello","protocol_version":1,"client_version":"socat","has_printer":false,"print_priority":0}}
{"id":2,"request":{"type":"list_boards"}}
```

The DEMON identifies you by the uid on the socket, there is nothing to log in with because I didnt want to make a log in system lol and this is good enough for me rn.

## Installing as a login shell

Add the `taskologic` binary path to `/etc/shells`, then set it as your taskologic users shell in `/etc/passwd`, and put the user in the `taskologic` group. The daemon runs as its own system user that owns the database file and the socket directory. Nobody else needs access to the database.

If you need telnet enable that on ur servers linux distro (usefull for when your old Windows CE tablet cant do modern ssh)

## Printing a task twice

It does not. A task prints automatically once per person, not once per move,
so pausing and unpausing, or sending it back to the to-do column and starting
it again, does not queue another slip. The daemon keeps a row per task and
person to remember. Pressing print yourself is a deliberate act and always
prints, however many times you ask.

## Updating an install

Once a host has been through `make setup`, new versions go on with:

```bash
make update
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
so this is not new; what `make update` adds is doing it **deliberately**, with
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
make update RESTART=no
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
