# Barcodes and the scanner

Taskologic prints a barcode on every slip and reads it back with any scanner that types like a keyboard (a "wedge" almost all USB ones should work, Bluetooth ones will work usually ... atleast most of the cheap ones). 

Scanning a task's code starts, pauses or finishes it without touching the screen. They are what are printed automatically on receipt slips for interacting with tasks with no mouse/touchsreen/keyboard and are considered system codes.

Codes that drive the client itself are called command codes and allow for overrides of what scanning the task code does, details on them are on their own page, [barcode-commands.md](barcode-commands.md).

## Setting a scanner up

Settings (`S`) has a scanner row:

- **Scanner enabled** is on by default. Off means nothing here applies :(
- **presses Enter**: tick it when the scanner sends Enter after the code. Most do. The client then swallows that Enter instead of opening whatever task is selected.
- **manual only**: only listen after the full width scan button (or `s`) is pressed. For a terminal where people also type a lot.
- **prefix**: characters the scanner is programmed to send before the code, if any. Usually empty and not possible on all scanners so thats the default too.
- **Barcodes**: the symbology printed slips use, CODE39 by default. CODE128 is denser and carries lowercase; QR and DataMatrix are for scanners that read 2D codes. What is scanned is read whatever it was printed as.
- **control codes wait**: seconds a control code waits for its next piece or its value; see [barcode-commands.md](barcode-commands.md).

The scanner itself needs no programming. It types characters; the client watches the stream for the code's shape. There is no timing involved, so a scanner over SSH from a slow link works the same as one on the box.

Print a test slip from Settings -> Printer -> Test print and scan it: the sample code toasts. The Print codes panel's "Scanner check" prints the same code twice, at normal and at thin bars, which tells you whether your scanner copes with one-dot bars.

## What is on a slip

A task code is `..1` + an action + the task's six character short id + a check character, eleven characters, so its always `..` in front and the `1` is a version string incase I ever change the commands. The check character catches a misread bar, a scan that does not add up is refused with a toast rather than acted on. (Unless its a controll command, those lack them for now)

The system barcode actions:

| code       | on the slip            | what a scan does                                                                |
|------------|------------------------|---------------------------------------------------------------------------------|
| `S`        | start / pause          | moves the task to the started column, or to paused if it is started already     |
| `F`        | finish                 | moves it to the finished column, dependencies permitting                        |
| `Y` / `N`  | yes / no               | answers a program step's yes/no question and finishes it                        |
| `1` to `8` | answer 1 to 8          | picks that answer of a choice question and finishes it                          |
| `C`        | finish what is running | on a program run's root: finishes every task of the run that is being worked on |

Which codes a slip carries is the slip layout's business ([printers.md](printers.md)): a **reminder** slip is laid out around the start code, the **task** slip around the finish codes (and the answer codes, when the task asks a question), and a program's **group sheet** lists every task of a fan out step with its start code plus the root's stop code. The **pause card** is a task's start/pause code on its own, printed by the `PP` control code.

A finish scan that hits open dependencies is refused and says so; the override is a deliberate move from the screen. A task prints automatically once per person, so pausing and restarting does not queue another slip; pressing print yourself always prints.

## Where a scan lands

On the board and the dashboard a task code does what it says. In a text field it is typed as text, so you can search for a short id by scanning a slip. Under any other window it beeps and asks you to close the window first, unless a control code is armed for the next scanned task, in which case that code acts on it wherever you are.

## Want to do more ?

Settings -> Print codes, You can print them one at a time or a whole category as a strip. For board specific ones go to the boards settings. For the Templates panel and the Programs panel each can also print a card with their own codes. 

All of that is explained in detail on [barcode-commands.md](barcode-commands.md).
