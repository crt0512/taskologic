# Printers

Everything about the receipt printer: the profile, what goes down the wire, why a printer might print nothing, and what a slip shows. 

The one-minute version is in [running.md](running.md).

## The printer profile

The printer lives in the client config, per user (per computer in the future once external api is created), under `[printer]`.

```toml
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
wide_barcodes = false        # true draws CODE39/CODE128 as wide as the paper
```

**You do not have to write any of this by hand**, the client has a setup panel:
Settings (`S`) -> Printer. It lists the queues `lpstat -e` reports, and
everything except the queue and the cutter lives behind Advanced, because
most printers do not care.

Picking a queue also asks CUPS what media that queue is set up for
(`lpoptions -p QUEUE`) and sets the paper width to match, so you do not have
to measure a roll (in most cases). 

As an example my Star TSP143 is reporting: `media=custom_71.97x199.67mm_...`
This lands on the 80 mm preset, which prints 72 mm wide. 
Receipt queues get named after the roll (58/80 mm) or after the
printable area (48/72 mm) depending on who set them up, both spellings of
the same roll land on the same preset unless you change the width by hand and yours.

### Print as: what the printer is actually fed

`output` is the setting that decides whether anything comes out at all.

| mode      | what we shove up the printers ass | barcodes                | needs                         |
|-----------|-----------------------------------|-------------------------|-------------------------------|
| `esc_pos` | ESC/POS commands                  | printer's own, or drawn | a printer that speaks ESC/POS |
| `bitmap`  | the slip drawn as a 1 bit PBM     | drawn here              | any queue CUPS can drive      |
| `text`    | plain text                        | **none**                | any queue CUPS can drive      |

**esc_pos** is the fast one. The printer sets the type with its own fonts,
draws its own barcodes and works its own cutter, and the job is about a
kilobyte. Pick it when you know the printer speaks ESC/POS.
(Mine kinda doesnt and I got rid of my Epson that did, good luck)

**bitmap** is the default, because it is the mode most likely to put ink on
paper on a printer nobody has told the client anything about yet. It draws
the whole slip here, at the paper's own dot width, and hands
CUPS a picture for the queue's driver to rasterise. Use it when the printer
has no fonts of its own. 

My **Star TSP100 / TSP143 is exactly this case**: 
it holds no font sets at all, so ESC/POS text sent to one is simply discarded,
the job completes, and nothing comes out. Bitmap is the mode that prints on
those, and unlike text it keeps the barcodes. The trade is size and speed:
the job is tens of kilobytes and the printer has to pull it all down.

Might crash an Epson on rare occasions.

**text** hands CUPS plain text and lets the driver set it. It is the simplest
thing that can work on a driver queue, it cannot draw barcodes however because
bars cannot be made out of characters at a density any scanner would read.

`rotation` turns the finished bitmap, and only bitmap output can do it: the
other two hand their work to something else to place. `cw180` is the one
receipt printers actually want, for a roll that feeds the other way round. A
quarter turn swaps the slip's width and height, so it only fits if the slip
is shorter than the roll is wide; past that the driver shrinks or clips it,
and the panel says so when you pick one.

A drawn slip is also kept a millimetre narrower than the paper it is named
after. Found that many dont like printing things the right way otherwise.

The panel greys out what a mode does not allow using. Character set and raw are
ESC/POS notions, so in the other two modes those rows explain themselves
rather than offering a control that does nothing, and `raw` is ignored
outright for a bitmap or text job, which exists to be filtered.

Barcodes have one setting of their own, live in both modes that draw them:
"as wide as the paper" (`wide_barcodes`). Off, a CODE39 or CODE128 is drawn
at two dots per bar, which is small and quick. On, the bars grow to the
biggest whole width that still fits the paper, quiet zones included, so the
code scans from further away and on worse paper; a printer drawing its own
codes is asked for the same, within the six dots its command allows. Square
codes keep their size either way, they would take the whole slip otherwise.

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
will take a raw job and then quietly abort it. 
If Test print reports success and no paper moves, that is the shape of it, check with:

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

The custom line is your own words, put whatever you want in there.

The layout lives in the printer profile, so it is per machine, and a slip
that prints wrong is one panel's fault rather than a hunt through
preferences. The old print switches in the settings screen are gone.

## Printing a task twice

It does not. A task prints automatically once per person, not once per move,
so pausing and unpausing, or sending it back to the to-do column and starting
it again, does not queue another slip. The daemon keeps a row per task and
person to remember. Pressing print yourself is a deliberate act and always
prints, however many times you ask.
