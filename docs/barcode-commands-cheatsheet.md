# Barcode commands: cheatsheet

The short version of [barcode-commands.md](barcode-commands.md). Print the codes from Settings -> Print codes; a code reads `--1<cmd>[/<cmd>]*--`, and `/` joins commands into one code.

## The basics

**Keys and screens** (CODE128)

| Code                                    | Does                                                                                    |
|-----------------------------------------|-----------------------------------------------------------------------------------------|
| `--1K<c>--`                             | presses the key `c`: `--1Kn--` is new task, `--1KT--` is Templates                      |
| `--1XE--`                               | Esc, also cancels whatever is armed or waiting                                          |
| `--1XN--`                               | Enter                                                                                   |
| `--1XT--` / `--1XB--`                   | Tab / Shift+Tab                                                                         |
| `--1XT3--` / `--1XB2--`                 | Tab three times / Shift+Tab twice: a count of 1 to 99 after any key letter (not F keys) |
| `--1XF1--` .. `--1XF12--`               | F1 to F12 (`--1XF2--` saves the task form)                                              |
| `--1XU--` `--1XD--` `--1XL--` `--1XR--` | arrow keys (moves the highlight on a board)                                             |
| `--1GD--`                               | all boards                                                                              |
| `--1GN--` / `--1GP--`                   | next / previous board                                                                   |
| `--1Q/V.soap--`                         | search for soap (`QA` includes the archive)                                             |
| `--1PING--`                             | toasts "scanner ok"                                                                     |
| `--1UNDO--`                             | undo the last change a code made to a task (see Undo below)                             |

**Typing into the focused field**

| Code                               | Does                      |
|------------------------------------|---------------------------|
| `--1I/V.hello--`                   | types hello at the cursor |
| `--1R/V.hello--`                   | replaces the whole field  |
| `--1R/X--`                         | empties the field         |
| `--1I/N--` `--1I/TD--` `--1I/ME--` | now, today, your username |

**Values** (alone, or joined with `/`)

| Value                 | Meaning                                                                                                                       |
|-----------------------|-------------------------------------------------------------------------------------------------------------------------------|
| `N` / `TD` / `TM`     | now / today (date only) / current time (time only)                                                                            |
| `P<n><u>` / `M<n><u>` | plus / minus n units on the field's value, or on now when it is empty; `H` `D` `W` `MO` `Y`, no unit is minutes: `P30`, `M2H` |
| `ME`                  | my username                                                                                                                   |
| `U`                   | then the due date: ends the start date's values in `SB`; a side left empty is left alone                                      |
| `X`                   | clear the field                                                                                                               |
| `V.<text>`            | free text, last in the frame                                                                                                  |

**Next scanned task.** These arm a command; the next task slip you scan (or `--1SEL--`, the task highlighted on screen) gets it instead of being started. One shot, unless printed sticky (`/STK`). End any of them with `/SEL` to act on the highlighted task in the same scan: `--1MR/SEL--`, `--1SB/P30/SEL--`.

| Code                                        | Does                                               |
|---------------------------------------------|----------------------------------------------------|
| `--1ML--` `--1MR--`                         | move left / right one column                       |
| `--1MU--` `--1MD--`                         | up / down one place (board must be open)           |
| `--1MT--` `--1MB--`                         | to top / bottom of the column (board must be open) |
| `--1MCT--` `--1MCP--` `--1MCD--` `--1MCF--` | to todo / paused / doing / done                    |
| `--1MC1--`..`--1MC9--`, `--1MCL--`          | to column number / the last column                 |
| `--1DEL--`                                  | archive, no confirmation                           |
| `--1AM--` `--1AU--` `--1AT--`               | assign me / unassign me / toggle                   |
| `--1PR--` `--1PF--` `--1PP--` `--1PC--`     | print reminder / finish / pause / both slips       |
| `--1SH--`                                   | open the task                                      |
| `--1SEL--`                                  | apply the armed command to the highlighted task    |

**Set a field on the next scanned task**: `--1S<field>--` opens the form on that field, `--1S<field>/<value>--` sets it straight away.
Fields: `T` title, `D` description, `S` start, `U` due, `B` start and due together, `RS` / `RU` reminder before start / due, `A` assign, `C` checklist (`SC/V.text` adds, `SC3` ticks item 3), `X` exclude from stats. `--1SU/X--` clears the due date.

**Boards, templates, programs** (printed where they live; `<id>` is the six character short id)

| Code                          | Does                                                 |
|-------------------------------|------------------------------------------------------|
| `--1GB<id>--` `--1GA<id>--`   | open the board / its analytics                       |
| `--1RP<id>--` `--1RN<id>--`   | program start dialog / start with defaults           |
| `--1NT<id>T--` `--1NA<id>T--` | task from template in todo now / open the form first |

## Make a task from barcodes

New task, title, one checklist item, and a 30 minute window that starts in 30 minutes: it starts 30 minutes from now and is due 30 minutes after that, an hour from now. With a task form open, a set code fills its field directly: no scan to wait for and no tabbing to the field. Scan in this order, with a board open:

| # | Scan                        | What happens                                                |
|---|-----------------------------|-------------------------------------------------------------|
| 1 | `--1Kn--`                   | new task form opens                                         |
| 2 | `--1ST/V.Buy soap--`        | title                                                       |
| 3 | `--1SC/V.Check the price--` | adds a checklist item (scan again for more)                 |
| 4 | `--1SB/P30/U/P1H--`         | start now plus 30 minutes, due now plus 1 hour, in one code |
| 5 | `--1XF2--`                  | F2, saves the task                                          |

The same works for every field: `SD/V.text` description, `SU/...` due, `SRS/...` and `SRU/...` reminders in minutes, `SB/P1H` start and due together, `SA/V.alice` assign (must be a board member, `SA/X` clears), `SC3` ticks checklist item 3. A set code with no value (`--1SU--`) just puts the cursor in that field. Offsets count from what the field holds, or from now when it is empty. Exclude from stats is not on the form. Tabbing (`--1XT3--`) is still there for anything a field code does not reach.

Shortcut, if a template with the wanted title, checklist and start already exists: `--1NT<id>T--` makes the task in todo in one scan.

## Push an existing task's start and due dates by 30 minutes

Offsets count from what the field holds, so a task due at 14:00 becomes 14:30 with plus 30 and 13:30 with minus 30. A field with nothing in it counts from now. `B` is start and due in one code, each counted from its own value.

Two ways to say which task, and each has a one-code form that ends in `/SEL` (the task highlighted on the board):

| Change         | A task slip: scan the code, then the slip | The highlighted task: one scan |
|----------------|-------------------------------------------|--------------------------------|
| +30 min, both  | `--1SB/P30--`                             | `--1SB/P30/SEL--`              |
| -30 min, both  | `--1SB/M30--`                             | `--1SB/M30/SEL--`              |
| +30 min, start | `--1SS/P30--`                             | `--1SS/P30/SEL--`              |
| -30 min, start | `--1SS/M30--`                             | `--1SS/M30/SEL--`              |
| +30 min, due   | `--1SU/P30--`                             | `--1SU/P30/SEL--`              |
| -30 min, due   | `--1SU/M30--`                             | `--1SU/M30/SEL--`              |

Highlight the task first with the arrow codes (`--1XD--` `--1XU--` `--1XR--` `--1XL--`). The code without `/SEL` arms, and the next slip scan, or `--1SEL--` afterwards, applies it. The code with `/SEL` arms and applies in the same scan. Each is one shot: scan again to nudge the same task again.

Printing the `/SEL` forms: in Settings -> Print codes -> Start and due dates, tick "on selected" (or press `t`), then pick "Set start and due, plus so much" (or "minus") and give 30 minutes: it prints `--1SB/P30/SEL--`. With sticky ticked too it prints `--1SB/P30/STK/SEL--`: it nudges the highlighted task now and stays armed, so every later `--1SEL--` nudges again, and Esc ends it.

**Different values for start and due.** Put `U` (then the due date) between the two: what comes before it is for the start, what comes after for the due date.

| Code                    | Does                                                                    |
|-------------------------|-------------------------------------------------------------------------|
| `--1SB/P30/U/P2D--`     | start plus 30 minutes, due plus 2 days                                  |
| `--1SB/N/U/TD/P1D--`    | start now, due tomorrow                                                 |
| `--1SB/P30/U--`         | start plus 30 minutes, due left alone                                   |
| `--1SB/U/M1H--`         | start left alone, due minus 1 hour                                      |
| `--1SB/X/U/P1W--`       | clear the start, due plus a week                                        |
| `--1SB/P30/U/M30/SEL--` | on the highlighted task: start 30 minutes later, due 30 minutes earlier |

Without a `U` both dates get the same value. The Print codes menu (Start and due dates) has four entries for two offsets, "start plus, due minus" and so on: it asks for the start's amount, then the due date's. `--1SB/P30/U/P2D--` is 17 characters, so on 58 mm paper it prints as CODE128.

To do one date only use `--1SU/P30--` (due) or `--1SS/P30--` (start). The Print codes menu has a "Start and due dates" category with these ready made for start, due and both: set to now, set to today, plus or minus so much (it asks how much), and clear.

To do a whole stack of slips, print the sticky variant (`--1SB/P30/STK--`, Print codes -> sticky box), scan it once, then scan every slip; Esc ends it.

## Undo

`--1UNDO--` takes back the last change a control code made to a task. Scan it again to walk back further, up to 3 changes.

| Undoes                                                                 | How                                                                |
|------------------------------------------------------------------------|--------------------------------------------------------------------|
| a move (`ML` `MR` `MU` `MD` `MT` `MB` `MC..`)                          | the task goes back to its old column and place                     |
| a set field or assign (`ST/..` `SB/..` `SU/..` `SS/..` `AM` `AU` `AT`) | the fields go back to what they were, both dates included for `SB` |
| a checklist tick (`SC3`) and exclude from stats (`SX`)                 | flipped back                                                       |
| delete / archive (`DEL`)                                               | restored from the archive                                          |

Only changes the daemon confirmed count: a code that failed or was refused leaves nothing to undo. It does not undo keys and typing (use Esc or `--1R/X--`), a new task made from the form or a template, moves made with the keyboard, or a printout. If someone else changed the task after your code, undoing an edit is refused as a conflict and the change stays; scan the correcting code instead. Undoing a move into a column with rules (doing, done, a program's dependencies) goes through the same checks as any move, so it can ask or be refused.

## Sticky reordering

Sticky keeps a command armed for every scan until Esc or the "control codes wait" timeout (20 seconds by default, and every scan resets it). Reordering needs the board open, because positions live in the board detail.

1. Open the board: `--1GB<id>--`, or `--1GD--` and pick it.
2. Highlight the task with the arrow codes: `--1XD--` / `--1XU--` between tasks, `--1XR--` / `--1XL--` between columns.
3. Scan the sticky move you want, printed with the sticky box ticked:
   - `--1MU/STK--` one place up, every time
   - `--1MD/STK--` one place down
   - `--1MT/STK--` `--1MB/STK--` to the top / bottom
   - `--1MR/STK--` `--1ML/STK--` one column over
4. Scan `--1SEL--` once per step: each one moves the highlighted task and leaves the command armed. The highlight follows the task to its new place, so the next `--1SEL--` moves the same task again. The status line reads "every scan: move up".
5. Esc (`--1XE--`) ends it.

The same sticky command works on slips: with `--1MD/STK--` armed, scan a stack of task slips and each one moves down a place instead of starting.

A move that cannot happen (already at the top, outermost column) toasts "it is there already" and leaves the command armed. `--1MC<col>--` and `--1DEL--` also come in sticky, so a whole stack can be sent to done or archived the same way.
