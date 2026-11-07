# openhantek-tui

A terminal oscilloscope for the **Hantek DSO-6022BE** USB scope. It talks to
the scope directly over USB (libusb), with the open firmware from
[OpenHantek6022](https://github.com/OpenHantek/OpenHantek6022), and draws
the traces in your terminal with [ratatui](https://ratatui.rs).

Features:

- Two channels and a math channel (CH1+CH2, CH1-CH2, CH2-CH1, CH1×CH2,
  CH1÷CH2, average, -CH1, -CH2)
- 20 mV/div to 10 V/div, probe x1 / x10, invert, DC / AC coupling
  (AC is done in software; a scope with the AC hardware mod also gets the
  hardware command)
- 20 ns/div to 10 s/div, roll mode from 100 ms/div
- Software trigger: Auto / Normal / Single, rising / falling / both edges,
  CH1 or CH2, level and position, with hysteresis against noise
- Measurements: Vpp, max, min, mean, RMS, AC RMS, amplitude, top, base,
  frequency, period, duty cycle, pulse widths, rise and fall time
- Four cursors (time A and B, voltage A and B), each with its own on/off,
  line style and colour; ΔT, 1/ΔT, ΔV and the value of each channel at
  the time cursors in the measurement panel
- Spectrum (FFT) view with Rectangular, Hann, Hamming, Blackman and flat top
  windows, in dBV, with peak readout and zoom
- XY mode
- Averaging, persistence (old traces fade by age), sin(x)/x interpolation
- Autoset
- Calibration output frequency from 32 Hz to 100 kHz
- Uses the calibration in the scope EEPROM and the OpenHantek6022
  calibration file, if you have one (it never writes the EEPROM)
- Export the screen as CSV or JSON; headless `--measure` mode for scripts
- Envelope logger (chart recorder): records min, max and mean per time
  step (2.5 µs to 1 min) without gaps, for hours or days, with a strip
  chart log view and CSV export
- Settings panel (`s`, `Enter` or `F2`) with yazi-style keys: every setting in one place,
  with search and typed values
- Clean braille plot: whole cells per division, so the grid lines up, and
  one-dot-thick traces; true colour, with 256 and 16 colour fallbacks
- One config file with every setting and every key binding, with a
  comment on each line; changes are saved on quit, in place, so your own
  comments and formatting stay
- Demo mode with a fake scope, so you can try it without hardware

## Run it

With Nix, from a clone (the flake brings the firmware along):

```
nix run .
nix run . -- --demo     # no scope needed
```

Or with cargo, in the dev shell:

```
nix develop
cargo run --release
cargo run --release -- --demo
```

Without Nix you need Rust, `pkg-config` and libusb 1.0. The scope also
needs its firmware file `dso6022be-firmware.hex` from OpenHantek6022
(`openhantek/res/firmware/`). Pass it with `--firmware FILE`, or put it in
`~/.config/openhantek-tui/dso6022be-firmware.hex`. You can also set
`OPENHANTEK_TUI_FIRMWARE=/path/to/file` when you build.

The firmware is loaded each time the scope gets power. That takes about
3 seconds: the scope then shows up again with a new USB ID (`04b4:6022`
becomes `04b5:6022`).

Close OpenHantek6022 first: only one program can use the scope.

### Options

```
--demo              fake scope with made-up signals
--firmware FILE     firmware file (Intel HEX)
--config FILE       config file (default ~/.config/openhantek-tui/config.toml;
                    --settings FILE is the old name and still works)
--no-settings       do not load or save the config file (also --no-config)
--export-dir DIR    folder for CSV and JSON exports (default: current folder)
--log-dir DIR       folder for envelope logs (default ~/.local/share/openhantek-tui/logs)
--open-log FILE     open this log in the log view
--autoset           run autoset at start
--measure           no screen: capture once, print the measurements, quit
--csv FILE          with --measure: also save the capture as CSV
```

Example:

```
$ openhantek-tui --measure --autoset
DSO-6022BE 0694CE656429 FW0210 · 500 µs/div · 2 MS/s · triggered
CH1: Vpp 2.13 V, Max 2.1 V, Min -24.9 mV, Mean 1.04 V, ..., Freq 1 kHz, Duty 50.0 %, ...
```

## USB permissions (udev)

Your user must be allowed to open the scope.

On NixOS, add the OpenHantek6022 udev rules:

```nix
services.udev.packages = [ pkgs.openhantek6022 ];
```

On other Linux systems, copy `utils/udev_rules/60-openhantek.rules` from
the OpenHantek6022 source to `/etc/udev/rules.d/`, then run
`sudo udevadm control --reload` and replug the scope.

## Keys

| Key | What it does |
|---|---|
| `q` `Esc` | quit (saves settings) |
| `?` `F1` | help |
| `s` `Enter` `F2` | settings panel (see below) |
| `space` | run / stop |
| `o` | single shot: arm the trigger once |
| `a` | autoset |
| `Tab` `Shift-Tab` | select CH1 / CH2 / MATH |
| `1` `2` `3` | CH1 / CH2 / MATH on or off |
| `↑` `↓` | volts/div of the selected trace |
| `[` `]` | move the selected trace down / up |
| `{` `}` | move the selected trace a little |
| `0` | selected trace to the center |
| `c` | coupling DC / AC |
| `p` | probe x1 / x10 |
| `i` | invert |
| `M` | next math function (turns MATH on) |
| `←` `→` | time/div |
| `t` | trigger mode: Auto / Normal / Single |
| `S` | trigger source CH1 / CH2 |
| `e` | trigger edge: rising / falling / both |
| `+` `-` | trigger level |
| `T` | trigger level to the middle of the signal |
| `,` `.` | trigger position left / right |
| `f` | spectrum (FFT) view |
| `F` | spectrum zoom (span / 2) |
| `w` | FFT window |
| `x` | XY mode |
| `m` | measurement panel |
| `v` | hide / show all cursors (each keeps its own on/off) |
| `h` `l` | move time cursor A (when it is on) |
| `H` `L` | move time cursor B (when it is on) |
| `j` `k` | move voltage cursor A (when it is on) |
| `J` `K` | move voltage cursor B (when it is on) |
| `Alt` + `h` `l` `H` `L` `j` `k` `J` `K` | move that cursor a hundredth of a division (fine) |
| `Alt-↑` `Alt-↓` | volts/div between the steps (fine) |
| `Alt-←` `Alt-→` | time/div between the steps (fine) |
| `Alt` + `[` `]` `{` `}` `+` `-` `,` `.` | a tenth of the normal step (fine) |
| `A` | averaging: 1 2 4 8 16 32 64 |
| `P` | persistence: 0 2 4 8 16 traces |
| `d` | dots / lines |
| `z` | sin(x)/x / linear interpolation |
| `g` `G` | calibration output frequency up / down |
| `E` | export the screen as CSV |
| `W` | export the screen as JSON |
| `R` | reset all settings |
| `r` | start / stop the envelope logger (see below) |
| `V` | log view on / off |
| `Ctrl-C` | quit |

**Fine steps with Alt.** `Alt` with a step key takes a small step:
the cursor keys move a hundredth of a division (less than one braille
dot), `[` `]` `{` `}` `+` `-` `,` `.` a tenth of their step, and in the
value box of a number setting `Alt-j` `Alt-k` step a tenth. `Alt-↑`
`Alt-↓` and `Alt-←` `Alt-→` give a volts/div or time/div between the
1-2-5 steps, in tenths of the gap to the next step (1 V, 1.1 V, 1.2 V
... 2 V, 2.3 V ...), shown like `1.1 V/div`. The scope then works at the
next bigger step: the hardware gain (so the input range covers the
screen) and the sample rate (so enough samples cover it) are those of
that step, and the plot, the measurements, the cursors, the trigger and
the logger use the exact value. A normal `↑` `↓` `←` `→` snaps back to
the 1-2-5 list. The value is saved as a factor on the step
(`volts_fine`, `units_fine`, `time_fine`; 1 = on the step), so older
files load as before. `Alt` works in every terminal, also VTE ones like
xfce4-terminal, and in tmux.

On screen: `1▸` and `2▸` on the left show each channel's 0 V line. `◂` on
the right is the trigger level, `▾` at the top the trigger position.
`CLIP CH1` in the title means the signal is bigger than the input range.

The input range shrinks when the volts/div goes down (more hardware
gain): at 20 to 100 mV/div the ADC covers only about ±0.5 V at the BNC.
A trigger level outside the range of the trigger source is never
crossed, so the trigger then uses a level 5 % of the range inside the
nearest edge (from the calibration, with probe, invert and AC coupling;
the hysteresis still applies). Your level does not change: the `◂` mark
turns red, the status line says
`Trig Auto CH1 ↑ 1 V (out of range, using 465 mV)`, and with a bigger
volts/div the real level is used again.
The labels on the left are the volts of the selected trace at each
division line; the labels below are the time from the trigger point.
The margins are kept small, so the plot gets the room: the left labels
are only as wide as the longest one, the `1▸` markers sit right next to
them, and the right side has one column for `◂` (two with fine dashes
voltage cursors, whose names go there).

## Config file

Every setting and every key binding is in one file,
`~/.config/openhantek-tui/config.toml` (or
`$XDG_CONFIG_HOME/openhantek-tui/config.toml`; `--config FILE` picks
another one, `--no-settings` uses none). When it is missing, the app
writes it at start: every setting with a short comment above it, then a
`[keys]` table with every action and its default keys. It starts like
this:

```toml
# openhantek-tui config. Every setting, then the key bindings.
# The app writes changed values back in place: your comments stay.
# A bad entry gives a warning and its default.

# time/div: index into 20ns 50ns 100ns ... 1s 2s 5s 10s (0..26, 14 = 1 ms)
time_idx = 14
...

# Key bindings: action = key or [keys]. ...
[keys]
# quit (saves the settings)
quit = ["q", "Esc"]
# key help
help = ["?", "F1"]
# settings panel
settings = ["s", "Enter", "F2"]
...
# run / stop
run_stop = "Space"
```

- **Saving.** Settings changed in the app are written on quit, and only
  the values that changed: the file is edited in place (with
  `toml_edit`), so your comments, the order and the formatting stay. If
  nothing changed, the file is not touched.
- **Bad entries.** A value of the wrong type or an unknown name (like
  `mode = "Sometimes"`) gives a warning on the message line and that
  setting keeps its default; the other entries still load. Unknown
  settings and actions also warn. All warnings are listed in Settings ›
  Files › Config warnings. The file is not rewritten: a change in the app
  later writes only that value, the bad entries stay as they are for you
  to fix. A file that is not TOML at all is never written.
- **Old settings.toml.** When there is no config.toml yet but a
  `settings.toml` from an older version is in the same folder, its
  values are taken once into the new config.toml. The old file is left
  in place, untouched; the app does not read it again.

**Keys.** `action = "key"` or `action = ["key", "key"]`; `[]` unbinds.
A key is a single character (`a`, `A`, `+`, `?`), or one of `Enter`
`Esc` `Tab` `Shift-Tab` `Space` `Backspace` `Delete` `Insert` `F1` to
`F12` `Up` `Down` `Left` `Right` `PgUp` `PgDn` `Home` `End`, with
`Ctrl-` or `Alt-` in front (`Ctrl-r`, `Alt-Left`). Shift is in the
character: `A` is Shift-a (`Shift-a` works too). Names are not case
sensitive. For example `settings = ["o", "F2"]` and `single = "O"`.
Actions are in four views: global (the scope and the log view), scope,
log view (which also takes the scope keys it does not use itself) and
the settings panel. A key bound to two actions of one view gives a
warning, and the first of them in the file keeps it (actions missing
from the file come after the ones in it). `Ctrl-C` always quits. Typing
a number or a search text in the panel uses fixed keys (characters,
`Enter`, `Esc`, `Backspace`, the arrows); in the value column of a
number, digits, `.` and `-` always start typing. The help screen (`?`)
and the key hints show the keys in use. The help lists the plain keys;
the Alt keys get one row, `Alt + key = fine step of that move/step
key`. Long texts wrap at word boundaries in their column. When the
help is taller than the screen (80x24) it is one column that scrolls
with `j` `k` (or `↓` `↑`), `PgUp` `PgDn`, `Home` `End`; the title shows
which rows are on screen, and any other key closes it. On a big
screen (120x40) it shows all keys in two columns.

**Ctrl and Alt in terminals.** `Alt` works in every terminal (the fine
cursor keys use it). `Ctrl` with a letter works too, but a terminal
without the kitty keyboard protocol (xfce4-terminal, GNOME Terminal and
other VTE terminals, tmux) sends Ctrl-Shift-h like Ctrl-h, and Ctrl-h
like Backspace in some setups, so bind such keys with care. When the
terminal supports the kitty protocol (kitty, foot, WezTerm, Ghostty,
Alacritty), the app turns it on (and off again on exit), and these keys
are all different.

## Settings panel

Press `s` (or `Enter`, or `F2`). The panel opens at the bottom of the screen, and the
live trace stays above it, so you see each change at once. It has three
columns, like the file manager yazi: categories, the settings in that
category with their values, and the possible values (or help) of the
setting under the cursor. The line above the key hints explains the
setting under the cursor. It sits on a thin rule across the panel
(`├── Show this channel ───┤`) in italic blue gray (cyan with 16
colours), so it looks like neither a setting row nor the title. Number settings (positions, trigger level and
position, cursor positions, calibration frequency) show their value in a
box in the third column instead of a list:

```
│ Level                                 100 mV │ ╭────────╮
│ Position                                50 % │ │ 100 mV │
│ Level to middle                            ⏎ │ ╰────────╯
│ Single shot                                ⏎ │ j k  ± 100 mV
│                                              │ type: 500m 2k 1.5u
```

In that column `j` `k` (or `↓` `↑`, `+`) step the value down / up, and a
digit, `.` or `-` starts typing a new value right there: `Backspace`
edits, `Enter` applies, and so does moving away (`←` `h` `↑` `↓`, or
closing the panel); `Esc` drops only the typed text. The calibration
frequency steps through its fixed steps, and a typed frequency takes the
nearest one. Settings with a fixed list (volts/div, styles, averaging)
keep the list.

The panel is only as high as its longest column needs: the 13
categories, the most settings in any category, or the longest list of
values, plus the title, the help line and the key hints (16 rows). It is
worked out over all settings, not the one under the cursor, so it never
changes size while you move. Value lists longer than 20 rows (time/div)
do not count; they scroll in the value column. On
a short screen the panel shrinks so a small plot stays above it, and its
columns scroll to keep the cursor in view.

| Key | What it does |
|---|---|
| `j` `k` `↓` `↑` | move down / up |
| `l` `→` `Enter` | go into the next column; in the value column: go back |
| `h` `←` | go back one column |
| `Esc` | go back one column; at the left: clear the search, then close |
| `Space` | toggle on/off, or the next value |
| `+` `-` | next / previous value; on numbers: one step up / down |
| digits `.` `-` | in the value column of a number: type it, like `500m`, `2k`, `1.5u`, `-0.2V`, `25%` |
| `=` `i` | on a number: type it, starting from the current value |
| `/` | search all settings as you type; `Enter` ends typing, `Esc` clears |
| `gg` `G` | first / last row |
| `s` `F2` `q` | close the panel (changes stay) |

Moving in the value column changes the setting at once, and it stays:
there is no Enter to confirm and no undo. Moving away or closing the
panel keeps it, and it is saved with the other settings. Offsets take
divisions, or volts when you type a `V` (like `1.5V`). Time cursors take
seconds, voltage cursors and the trigger level take volts, the trigger
position takes percent. Every setting with a key is in the panel, and so
are run/stop, single shot, autoset, export and reset. The Device category
shows the model, serial number, firmware, calibration source and sample
rate. Display › Colours picks true colour, 256 or 16 colours (`auto`
uses `COLORTERM`).

Each trace has a Style (Channel 1, Channel 2 and Math › Style), used in the
time view and the spectrum:

| Style | Look |
|---|---|
| `braille thin` (default) | lines from point to point through the low and high value of each dot column; each column is one run of dots that starts next to where the column before ended, so edges and steep flanks stay one dot wide |
| `braille bold` | one dot run per column, two dots thick |
| `ascii lines` | no braille, for fonts that draw it badly: one character per cell, `\|` on steep runs, `/` and `\` on slopes, `_` `-` `‾` for flat parts at the bottom, middle or top of the cell; dots mode puts `.` `·` `'` per sample |
| `box lines` | no braille: one connected line of box-drawing characters, which the terminal joins cell to cell. `─` for flat runs at the level of the cell row, `│` for steep runs and noise, `╭` `╮` `╰` `╯` where the line turns, `╵` `╷` where a run stops in a cell; dots mode puts `•` per sample |

Dots (`d`), persistence (`P`) and interpolation
(`z`) work with every style. Settings files from older versions still
load: the old style names `Classic` and `Thin` read as `braille thin`,
`Bold` as `braille bold`.

The grid and the center cross (the axes through 0) have three settings
each under Display, used in the time view, the spectrum and XY. They are
independent: for example no grid and a solid center cross.

| Setting | Values |
|---|---|
| Grid style | `dots` (default: `·` where division lines cross), `dashed` (`╌` `╎`), `solid` (`─` `│` with `┼` crossings), `crosses` (`+` where lines cross), `none` (no grid) |
| Grid lines | `points` (default: marks only at the crossings, short arms for dashed and solid) or `full` (every division line across the whole plot, with `┌ ┬ ├ ┼` joins) |
| Grid brightness | 10 % to 100 % in steps of 10 (default 30 %, the old look) |
| Axis style | `dashed` (default: `╌` `╎` with `┼` and `┴` `├` ticks), `solid` (`─` `│`), `dots` (`·`), `crosses` (`+` at the divisions, `·` between), `none` (the grid shows on the center lines) |
| Axis lines | `full` (default: the whole center lines) or `points` (only the division points and the 1/5 division ticks, short arms for dashed and solid) |
| Axis brightness | 10 % to 100 % in steps of 10 (default 30 %; the center cross is a step brighter than the grid at the same percentage) |

The defaults draw the center cross of the first versions. A config
file from before the axis settings keeps its look: with `grid_style =
"Solid"` the center cross is solid, with `"None"` it is off. The axes
are drawn over the grid, and traces over both. Ticks at 1/5 division
show when a division is a multiple of 5 cells and at least 10.
Brightness scales the perceptual lightness in true colour, takes the
nearest xterm gray in 256 colours, and has four steps in 16 colours
(dim dark gray, dark gray, gray, white).

Persistence (`P`, time view only) keeps 2, 4, 8 or 16 old traces, and
each fades by age: the newest old trace is at 85 % of the way from the
background to the trace colour, the oldest at 12 %, in even perceptual
steps. A newer trace wins a cell over an older one, and the live trace
wins over all. In 256 colours the fade uses the nearest xterm colour of
the same hue (about 5 steps); in 16 colours the newer half is the normal
colour and the older half also has the dim attribute. With box lines, old
traces of different ages still join where they cross (the joined cell
takes the newer colour), so they stay connected lines.

## Measurement panel

`m` shows the panel at the right of the plot: every measurement of each
trace that is on (CH1, CH2, MATH), and a Cursors section when a cursor is
on. The Cursors heading is right-aligned over the values like the CH1 /
CH2 headings, and heading and values have one colour: the cursor colour
when all cursors that are on share it, else the palette cursor gray.
Its layout depends only on the screen size and on which traces and
cursors are on, never on the values, so it does not jump:

- The narrowest layout that shows every row of every trace wins, so the
  plot keeps the most width. First stacked: one value column, a section
  per trace under each other (heading in the trace colour). If that is
  too high, two value columns (CH1 and CH2 side by side, MATH below
  them), then three, but only while the plot keeps at least 80 columns.
- When nothing fits, the panel uses the widest layout it may, and fills
  its rows in this order: the selected trace (`Tab`) in full, the Cursors
  section, then the other traces in the order CH1, CH2, MATH. What does
  not fit is cut at the bottom.

For example, with CH1 and CH2 on, 120x40 is stacked and 80x24 shows the
selected channel and as much of the other one as fits; with MATH on too,
120x40 puts CH1 and CH2 side by side with MATH below. The panel is 20
columns wide with one value column (11 more per column), and needs 40
columns of plot next to it, or it is not shown.

## Cursors

There are four cursors: time A and B (vertical lines) and voltage A and B
(horizontal lines, in volts of the selected trace). Each has its own
setting in Settings › Cursors: on/off with its position (`Time A`: Space
turns it on or off, `j` `k` or a typed value move it, and typing a
position turns it on), a line style and a colour. That is 12 settings,
so the category is no higher than the others. All are off at first.

| Style | Look |
|---|---|
| `classic` (default) | dashes of a fixed length, the name inside the plot |
| `fine dashes` | short dashes on every cell, the name in the margin |
| `solid` | a solid line, the name inside the plot |
| `dotted` | one dot per cell (every second cell across), the name inside the plot |

Colours: `grey` (default: the grey line and white name of the first
versions), white, yellow, cyan, magenta, green, red, blue. The plot shows
only the letter `A` or `B` next to each line. `v` hides all cursors and
shows them again, each with its own on/off; `h` `l` `H` `L` `j` `k` `J`
`K` move only the cursors that are on. The values are in the Cursors
section of the measurement panel: `A t` `B t` (time from the trigger
point), `ΔT` and `1/ΔT` when both time cursors are on, `A v` `B v`, and
`ΔV` when both voltage cursors are on; and `V@A` `V@B`, the value of each
channel at a time cursor. The log view uses the style and colour of time
cursors A and B. Settings files from older versions still load: the old
cursor mode `Time` turns time cursors A and B on, `Volt` the voltage
cursors, and the old global cursor style becomes the style of each
cursor.

## Envelope logger

The logger is a chart recorder: it streams the scope without a break and
saves, for each time step and each channel that is on, the lowest, the
highest and the mean voltage. The volts are the ones the scope view
shows: calibrated, with the probe factor and invert (AC coupling is not
applied; the log is always DC). Press `r` to start and `r` again to stop
(or Logger › Record in the settings panel). The log uses the volts/div
(the input range), probe and channels set at the start.

While it records, the scope streams at the ADC rate the logger needs and
the scope view pauses (its title says `● REC: live view paused`);
changes of volts/div or time/div only change the drawing, and reach the
scope after the log stops. The log view (`V`) shows the log growing.

**Step and ADC rate.** Steps from 2.5 µs to 1 min in 1-2-5 steps (and
30 s). The ADC runs at the lowest of 20, 40, 100, 200 or 400 kS/s that
gives at least 10 samples per step, and never faster than 400 kS/s, the
rate the scope streams without losing samples. At 2.5 µs the step is one
sample at 400 kS/s, so only the value is saved (min = max = mean); 5,
10 and 20 µs have 2, 4 and 8 samples per step.

| Step | ADC rate | Samples per step | File size per hour, CH1 + CH2 | one channel |
|---|---|---|---|---|
| 2.5 µs | 400 kS/s | 1 | 11.5 GB | 5.8 GB |
| 10 µs | 400 kS/s | 4 | 8.6 GB | 4.3 GB |
| 100 µs | 100 kS/s | 10 | 864 MB | 432 MB |
| 1 ms | 20 kS/s | 20 | 86.4 MB | 43.2 MB |
| 1 s | 20 kS/s | 20 000 | 86 kB | 43 kB |

**Gaps.** The thread checks that the samples keep up with the clock. When
they fall more than 0.3 s behind (the scope lost samples), or the stream
had to start again (a USB error, the scope unplugged and back), the log
gets a gap mark and goes on at the right time. Smaller losses add up
until they reach 0.3 s. The log view draws a gap as a break with a red
dashed line `┊`; the status line counts them.

**Size limit.** Logger › Size limit (`log_limit`, 10 MB to 50 GB, or
off) is the only limit: the log stops by itself when the file reaches
it, and the message line says so. The panel shows how long that is,
like `Limit 1 GB ≈ 11 h 34 min at 1 ms step` (from the step and the
channels that are on), and warns when the disk has less free space than
the limit.

**Status line.** While recording:

```
 ● REC  412 MB / 1 GB   0:41:10, about 6:48:07 left   free 120 GB
```

the file size and limit, the time since the start, the time left at the
log's data rate, and the free space of the log folder. When the disk would
fill before the limit it says `disk full in about ...` on a yellow
background. `N gaps` and `CLIP` (a sample at the end of the ADC range)
show up when they happen.

**Log view.** `V` switches between the scope and the log view (the last
log, or the one being recorded). Settings › Logger › Open log (also in
Files) lists the 10 newest logs of the log folder; `--open-log FILE`
opens any log. In that list `d` (`[keys] delete_log`) deletes the log
under the cursor: a box asks `Delete log-20261107-185153.ohlog (2.3 MB)?
y / n`, and only `y` deletes the `.ohlog` and its `.idx` summary file
(`n`, `Esc` or any other key keeps it). The log being recorded is never
deleted (stop it first); a log open in the log view is closed first. For each channel the band from min to max is filled (a
dim colour of the channel), and the mean is a line in the channel's
trace style: braille thin or bold (dots), ascii lines (`:` fill) or box
lines (`░` fill). The grid settings apply. Volts/div, positions and
channels on/off work as in the scope view (`↑` `↓` `[` `]` `{` `}`
`Tab` `1` `2`).

| Key | What it does |
|---|---|
| `←` `→` | scroll one division |
| `PgUp` `PgDn` | scroll one screen |
| `Home` `End` | start of the log / the newest data (and follow it) |
| `+` `-` | zoom in / out (1-2-5 steps per dot column, then 2 5 10 20 ... dot columns per step) |
| `F` `n` | the whole log / one step per dot column |
| `v` | time cursors A and B on / off |
| `h` `l`, `H` `L` | move cursor A, cursor B (`Alt`: a tenth, at least one step) |
| `t` | time labels: clock (UTC, with the date when the screen spans two days) or seconds since the start |
| `E` | export CSV: between A and B when the cursors are on, else the whole log |

Past one step per dot column `+` goes on: each step is drawn over 2,
5, 10, 20 ... dot columns, until about 10 steps fill the screen. Each
step is then a flat band from its min to its max, and the mean is a
line through the middles of the steps; logs of single samples (2.5 µs
to 10 µs steps) have no band, the line joins the samples. Time labels
and the `/div` in the status line follow (like `1.1 ms/div`, with an
extra decimal so the labels keep even steps), cursors sit in the middle
of their step, and every cursor key (`Alt` too) moves at least one
step, so a cursor can be put on each sample.

With the cursors on, the first status line shows A, B, ΔT, and the min,
max and mean of each channel between them. Zoomed out, each dot column
is the min of the mins, the max of the maxes and the mean of the means of
its steps. A summary (min/max/mean of every 1024 steps, and every 16
entries above that) makes this fast for big files; it is built when a
log opens (the title shows `summary 45%` meanwhile, at a few hundred MB
per second) and saved next to the log as `<log>.idx`, so the next open
is at once (a 2 GB log: 5 s to build, then 0.03 s to open and draw).

**CSV export.** `E` in the log view, or Logger › Export log CSV / Export
A–B CSV, writes `openhantek-log-YYYYMMDD-HHMMSS.csv` in the export
folder. Columns: the time of the start of each step, then
`ch1_min,ch1_max,ch1_mean,ch2_min,...` for the logged channels (a 2.5 µs
log has `ch1,ch2`). Logger › CSV time picks `time_utc` (ISO 8601 UTC,
like `2026-11-07T18:51:53.613Z`, with as many decimals as the step needs)
or `time_s` (seconds since the start). Each gap is one row with the
time where the data stops and empty value cells, like `8.000,,,,,,`, so
plotting tools break the line there.

**Settings** (the `[logger]` table of config.toml):

| Setting | Meaning |
|---|---|
| `step` | step in seconds (default 0.001) |
| `log_limit` | size limit in bytes, 0 = off (default 1000000000) |
| `log_dir` | log folder (default `$XDG_DATA_HOME/openhantek-tui/logs` or `~/.local/share/openhantek-tui/logs`; `--log-dir` wins) |
| `time_axis` | `Clock` or `Seconds` |
| `csv_time` | `Iso` or `Seconds` |

**File format** (`.ohlog`, little-endian). Logs are named
`log-YYYYMMDD-HHMMSS.ohlog` (UTC).

```
header   "OHTLOG\0\x01", version u16 (1), header length u16, flags u16,
         channel mask u8 (bit 0 CH1, bit 1 CH2), reserved u8,
         start time i64 (ns since 1970 UTC, the first sample),
         step u64 (ns), ADC rate f64 (S/s), samples per step u32,
         reserved u32,
         per channel (CH1, CH2, 24 bytes): gain index u8, probe u8,
         invert u8, DC u8, volts/div f32, ADC count of 0 V f32,
         volts per ADC count f32 (at the BNC), reserved 8 bytes,
         device name (u16 length + UTF-8), zeros up to the header length
chunks   "CHNK", flags u32 (bit 0: data was lost before this chunk),
         first step u64, record count u64 (u64::MAX: open, the records go
         to the end of the file), then the records
record   per logged channel: min, max, mean as f32 volts (one f32 when
         there is one sample per step)
```

Step `i` starts at `start + i * step`. A new chunk starts only after a
gap, so a log is usually one chunk. Missing step numbers between chunks
are the gap. The writer thread buffers in memory, hands the data to the
OS 4 times a second (the log view reads it from there) and fsyncs every
2 s; the record count of the open chunk is written when it closes. A
killed program leaves a file that reads up to the last write (a cut
record at the end is ignored).

## How it works

- `src/device/`: USB. `protocol.rs` has the vendor requests, sample rates,
  gains and calibration math; `hantek.rs` is the driver; `firmware.rs` and
  `hex.rs` load the firmware; `usb.rs` finds and opens the scope;
  `fake.rs` is a fake scope that acts like the firmware.
- `src/acquire.rs`: a thread that captures blocks and sends them to the UI.
- `src/scope.rs`: trigger, averaging, math, roll mode, autoset.
- `src/dsp/`: trigger search, measurements, FFT, math, interpolation.
- `src/logger/`: the envelope logger: `format.rs` (file layout),
  `binner.rs` (min/max/mean per step), `writer.rs` (writer thread, size
  limit), `reader.rs` (reads growing files), `pyramid.rs` (summaries and
  the `.idx` cache), `csv.rs`, `view.rs` (zoom, scroll, cursors).
  `src/keys.rs` maps the logger keys to actions; `src/ui/log.rs` draws
  the log view and the REC status.
- `src/app.rs` and `src/ui.rs`: keys and drawing. `src/plot.rs` is the
  braille and text renderer (trace styles and cursor lines), `src/theme.rs` the colours, `src/panel.rs` the
  settings panel model.

Like OpenHantek6022, it reads 20000 samples per channel per block, throws
away the first samples after each start (the ADC is not stable yet), and
averages several ADC samples into one at slow time bases. The fastest rates
are 24 MS/s for CH1 alone and 12 MS/s with both channels. Roll mode and the
logger run the ADC at most at 400 kS/s, so the scope buffer does not
overflow between USB reads.

## Tests

```
cargo test                    # unit, end-to-end (fake scope) and UI tests
OHT_BIG_MB=2000 cargo test --release --test logger big -- --ignored --nocapture
                              # speed of a big log (written to the temp folder)
cargo test --test hardware -- --ignored --test-threads=1 --nocapture
```

The hardware tests need the real scope. With a probe on the calibration
output they also check the frequency and the voltage.

## Licence of the firmware

The scope firmware comes from OpenHantek6022 (GPL-3.0). This repository
does not contain it: the program reads the firmware file at run time. The
Nix flake points at the file in the `openhantek6022` source from nixpkgs.
