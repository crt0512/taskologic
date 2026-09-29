# Barcode commands: control codes

Task slips carry system codes: `..`, a task, an action and a check, one scan does one thing to one task ([barcodes.md](barcodes.md)). 

Control codes are the other kind, framed by `--` on both ends, and they can do ... lets say ... alot more. Inteded for the really autisticly challanged people (me)

## What they do, by example

- **Keys and screens.**
  - `--1KT--` presses `T`
  - `--1XE--` is Esc
  - `--1GD--` shows all boards
  - `--1GN--` and `--1GP--` step through the boards
  - `--1Q/V.soap--` searches for soap.
- **Typing.** 
  - `--1I/V.hello--` types hello into whatever field has the focus
  - `--1I/N--` types now as a date and time, `--1I/TD--` today
  - `--1I/ME--` your username
  - `--1I/N/P2H--` two hours from now
  - `--1R/X--` empties it.
  - Insert or replace on its own waits for the value on the next scan.
- **The next scanned task.**
  - `--1MR--` then any task's code moves that task one column right instead of starting it
  - `--1MCF--` moves it to done
  - `--1MC2--` to the second column
  - `--1DEL--` archives it
  - `--1PP--` prints its pause card
  - `--1AM--` assigns it to you
  - `--1SH--` opens it. The status line says what is armed
  - `--1SEL--` applies the armed command to the task highlighted on screen instead of a scan. 
  - A code printed sticky (`/STK`) stays armed for every scan until Esc or the timeout.
  - A "move to" without a column (`--1MC--`) asks on screen, columns numbered; a digit or Enter picks.
  - Moving up, down, to the top or bottom of a column needs that board open.
- **Setting a field.**
  - `--1SU/N/P2H--` then a task code sets its due date to two hours from now
  - `--1ST--` alone opens the task form on the title
  - `--1SA/V.alice--` assigns alice
  - `--1SA/X--` clears the assignees
  - `--1SC/V.Buy soap--` adds a checklist item
  - `--1SC2--` ticks the second
  - `--1SX--` toggles exclude from stats.
  - Offsets count from what the field holds, or from now when it holds nothing.
- **Boards, templates and programs.**
  - Each has a six character short id like tasks.
  - `--1GB<id>--` opens the board
  - `--1GA<id>--` its analytics
  - `--1RP<id>--` opens a program's start dialog
  - `--1RN<id>--` starts it with its defaults
  - `--1NT<id>T--` makes a task from a template in todo straight away
  - `--1NA<id>T--` opens the form first
  - The cards for these print from where the thing lives, which would be: 
    - Board settings
    - Templates panel
    - Programs panel
  - The ids survive a server move, since export and import keep them ([transfer.md](transfer.md)).
- **A check.** `--1PING--` toasts "scanner ok".

**Long codes** may come on several barcodes: Enter between the pieces is ignored until the closing `--`. The status line says a long code is being read; Esc drops it. Settings has "control codes wait": how many seconds a code waits for its next piece, its value, a task scan for an armed command, or an answer on screen; 0 waits forever, and every scan resets it.

## Printing them

Settings -> Print codes. Pick a category, Enter prints the highlighted code on a card, `a` prints the whole category as one strip. An entry that needs something first asks in a popup: which key, what text, which column, plus or minus how much in what. `c` combines the highlighted command with a value into one code ("insert now, plus 2 hours" prints as `--1I/N/P2H--`), and the sticky box prints a next scan code as the variant that stays armed until Esc or the timeout.

A code prints in CODE39 when it fits the configured paper at the usual bar width and in CODE128 when it does not, keys and free text are always CODE128, so a scanner has to read CODE128 for those. On 58 mm paper about twelve characters of CODE39 fit at normal bar width, fifteen of CODE128, on 80 mm about twenty and twenty-four.

That is why the codes are spelt tersely: the label above the code is what you read, the payload under it is for checking.
The scanner itself needs nothing: the frames are plain characters.

## The shape of a control code
```pre
--1<cmd>[/<cmd>]*--

--        opening frame
1         format version, one character of insurance, as on system codes
cmd       a verb of one to three uppercase letters, with its arguments attached
/         joins commands into one code (the menu's "combine")
--        closing frame
```
Spelling is terse on purpose: every character is bar width, and the label printed above a code is what a person reads; the payload line under it is for comparing against what the scanner typed when something goes wrong.

Widths that follow: `--1MCF--` (move to done) is 8 characters and prints in CODE39 anywhere. `--1SU/N/P2H--` (due in two hours) is 13: CODE39 on 80 mm, CODE128 on 58. `--1GBK4M9Q2--` (show a board) is 13, so same ruling. Keys and free text are CODE128 always because we want to be able to print UPPER and lower case letters.

## Values
Anything that waits for a value takes one of these, alone or combined:

| Value                | Meaning                                                                                                                                                                                         |
|----------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `N`                  | now, date and time                                                                                                                                                                              |
| `TD`                 | today, date only; a time already in the field is kept                                                                                                                                           |
| `TM`                 | the current time, time only; a date already in the field is kept                                                                                                                                |
| `P<n><u>`, `M<n><u>` | plus or minus n units, applied to the field's current value, or to now when it has none; units `H` hours, `D` days, `W` weeks, `MO` months, `Y` years, none means minutes: `P30`, `M2H`, `P1MO` |
| `ME`                 | my username                                                                                                                                                                                     |
| `X`                  | clear the field                                                                                                                                                                                 |
| `V.<text>`           | free text, last in the frame                                                                                                                                                                    |

A token that starts with `P` or `M` followed by a digit is an offset; verbs never have a digit in second place, so the two cannot be confused.

## Every code
Labels are what the slip says above the code. Payloads are shown without the frame and version: `MCF` prints as `--1MCF--`. Lengths in brackets are the whole printed payload.

### Next scanned task (arms an override; add `/STK` for sticky)
| Label                                   | Payload                    | Notes                                                                                                  |
|-----------------------------------------|----------------------------|--------------------------------------------------------------------------------------------------------|
| Move left / right                       | `ML`, `MR` (7)             | one column over                                                                                        |
| Move up / down                          | `MU`, `MD` (7)             | one place in the column                                                                                |
| Move to top / bottom                    | `MT`, `MB` (7)             |                                                                                                        |
| Move to column                          | `MC<col>` (8)              | `MCT` todo, `MCP` paused, `MCD` doing, `MCF` done, `MC1`..`MC9`, `MCL` last; `MC` alone asks on screen |
| Delete (archive)                        | `DEL` (8)                  | no confirmation                                                                                        |
| Print reminder / finish / pause / combo | `PR`, `PF`, `PP`, `PC` (7) | reminder slip, task slip, the fixed pause card, both slips                                             |
| Assign me / unassign me / toggle        | `AM`, `AU`, `AT` (7)       |                                                                                                        |
| Set field                               | `S<field>` (7 or 8)        | opens the task form on the field; see fields below                                                     |
| Set field to value                      | `S<field>/<value>`         | sets it straight away, no form                                                                         |
| Show task                               | `SH` (7)                   | opens the next scanned task's view                                                                     |
| Selected element                        | `SEL` (8)                  | completes the armed override on the highlighted task                                                   |

Fields: `T` title, `D` description, `S` start, `U` due, `RS` reminder before start, `RU` reminder before due (minutes), `A` assign (a username as the value, `SA/V.alice`; `SA/X` unassigns), `C` checklist (`SC/V.text` adds an item, `SC3` ticks item 3), `X` exclude from stats (toggle). Clearing is the value `X`: `SU/X` clears the due date.

### Navigation and keys (always CODE128)
| Label                         | Payload                     | Notes                                                               |
|-------------------------------|-----------------------------|---------------------------------------------------------------------|
| Press a key                   | `K<c>` (7)                  | one character, case as printed: `KT` is Templates, `Kn` is new task |
| Up / Down / Left / Right      | `XU`, `XD`, `XL`, `XR` (7)  |                                                                     |
| Esc / Enter / Tab / Shift+Tab | `XE`, `XN`, `XT`, `XB` (7)  | `XE` also cancels whatever is armed or waiting                      |
| Dashboard (all boards)        | `GD` (7)                    |                                                                     |
| Next / previous board         | `GN`, `GP` (7)              | in dashboard order; no key does this today                          |
| Search                        | `Q/V.<text>`, `QA/V.<text>` | the second includes the archive                                     |
| Scanner check                 | `PING` (9)                  | toasts "scanner ok", printed on the test strip at two widths        |

Keys are injected into the same path an unmatched key takes today, so whatever the screen would do with the key, it does.

### Data entry (the focused field)
| Label   | Payload     | Notes                                                              |
|---------|-------------|--------------------------------------------------------------------|
| Insert  | `I/<value>` | at the cursor: `I/N` now, `I/TD` today, `I/ME` my name, `I/V.text` |
| Replace | `R/<value>` | the whole field                                                    |

### Values (for anything waiting)
`N`, `TD`, `TM`, `P<n><u>`, `M<n><u>`, `ME`, `X`, `V.<text>`, as in the table above; combinable: `N/P2H` is now plus two hours, `TD/P1D` tomorrow.

### Targeted (printed where the thing lives)
| Label                       | Payload             | Notes                                   |
|-----------------------------|---------------------|-----------------------------------------|
| Show board                  | `GB<sid>` (13)      |                                         |
| Analytics of board          | `GA<sid>` (13)      |                                         |
| Start program               | `RP<sid>` (13)      | opens the start dialog                  |
| Start program now           | `RN<sid>` (13)      | starts with defaults                    |
| New task from template      | `NT<sid><col>` (14) | creates it in that column straight away |
| New task from template, ask | `NA<sid><col>` (14) | opens the task form prefilled instead   |
