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
- Time and voltage cursors with ΔT, 1/ΔT and ΔV
- Spectrum (FFT) view with Rectangular, Hann, Hamming, Blackman and flat top
  windows, in dBV, with peak readout and zoom
- XY mode
- Averaging, persistence (old traces stay dimmed), sin(x)/x interpolation
- Autoset
- Calibration output frequency from 32 Hz to 100 kHz
- Uses the calibration in the scope EEPROM and the OpenHantek6022
  calibration file, if you have one (it never writes the EEPROM)
- Export the screen as CSV or JSON; headless `--measure` mode for scripts
- Settings panel (`S`) with yazi-style keys: every setting in one place,
  with search and typed values
- Clean braille plot: whole cells per division, so the grid lines up, and
  one-dot-thick traces; true colour, with 256 and 16 colour fallbacks
- Settings are saved on quit
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
--settings FILE     settings file (default ~/.config/openhantek-tui/settings.toml)
--no-settings       do not load or save settings
--export-dir DIR    folder for CSV and JSON exports (default: current folder)
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
| `S` `F2` | settings panel (see below) |
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
| `m` | next math function (turns MATH on) |
| `←` `→` | time/div |
| `t` | trigger mode: Auto / Normal / Single |
| `s` | trigger source CH1 / CH2 |
| `e` | trigger edge: rising / falling / both |
| `+` `-` | trigger level |
| `T` | trigger level to the middle of the signal |
| `,` `.` | trigger position left / right |
| `f` | spectrum (FFT) view |
| `F` | spectrum zoom (span / 2) |
| `w` | FFT window |
| `x` | XY mode |
| `M` | measurement panel |
| `v` | cursors: off / time / voltage |
| `h` `l` | move time cursor A |
| `H` `L` | move time cursor B |
| `j` `k` | move voltage cursor A |
| `J` `K` | move voltage cursor B |
| `A` | averaging: 1 2 4 8 16 32 64 |
| `P` | persistence: 0 2 4 8 16 traces |
| `d` | dots / lines |
| `z` | sin(x)/x / linear interpolation |
| `g` `G` | calibration output frequency up / down |
| `E` | export the screen as CSV |
| `W` | export the screen as JSON |
| `R` | reset all settings |
| `Ctrl-C` | quit |

On screen: `1▸` and `2▸` on the left show each channel's 0 V line. `◂` on
the right is the trigger level, `▾` at the top the trigger position.
`CLIP CH1` in the title means the signal is bigger than the input range.
The labels on the left are the volts of the selected trace at each
division line; the labels below are the time from the trigger point.

## Settings panel

Press `S` (or `F2`). The panel opens at the bottom of the screen, and the
live trace stays above it, so you see each change at once. It has three
columns, like the file manager yazi: categories, the settings in that
category with their values, and the possible values (or help) of the
setting under the cursor.

| Key | What it does |
|---|---|
| `j` `k` `↓` `↑` | move down / up |
| `l` `→` `Enter` | go into the next column; on a value: keep it |
| `h` `←` | go back one column; in the value column this undoes the change |
| `Esc` | go back one column (undoes in the value column); at the left: clear the search, then close |
| `Space` | toggle on/off, or the next value |
| `+` `-` | next / previous value; on numbers: one step up / down |
| `=` `i` | type an exact number, like `500m`, `2k`, `1.5u`, `-0.2V`, `25%` |
| `/` | search all settings as you type; `Enter` ends typing, `Esc` clears |
| `gg` `G` | first / last row |
| `S` `F2` `q` | close the panel (changes stay) |

Moving in the value column changes the setting at once. Offsets take
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
| `classic` (default) | lines from point to point through the low and high value of each dot column, like the first version |
| `braille thin` | one braille dot thick, one dot run per column |
| `bold` | the thin line, two dots thick |
| `ascii lines` | no braille, for fonts that draw it badly: one character per cell, `\|` on steep runs, `/` and `\` on slopes, `_` `-` `‾` for flat parts at the bottom, middle or top of the cell; dots mode puts `.` `·` `'` per sample |

Display › Cursor style (also under Cursors) picks `classic` (grey dashes,
`A`/`B` inside the plot, the default) or `fine dashes` (short dashes,
names in the margin). Dots (`d`), persistence (`P`) and interpolation
(`z`) work with every style.

## How it works

- `src/device/`: USB. `protocol.rs` has the vendor requests, sample rates,
  gains and calibration math; `hantek.rs` is the driver; `firmware.rs` and
  `hex.rs` load the firmware; `usb.rs` finds and opens the scope;
  `fake.rs` is a fake scope that acts like the firmware.
- `src/acquire.rs`: a thread that captures blocks and sends them to the UI.
- `src/scope.rs`: trigger, averaging, math, roll mode, autoset.
- `src/dsp/`: trigger search, measurements, FFT, math, interpolation.
- `src/app.rs` and `src/ui.rs`: keys and drawing. `src/plot.rs` is the
  braille and text renderer (trace styles and cursor lines), `src/theme.rs` the colours, `src/panel.rs` the
  settings panel model.

Like OpenHantek6022, it reads 20000 samples per channel per block, throws
away the first samples after each start (the ADC is not stable yet), and
averages several ADC samples into one at slow time bases. The fastest rates
are 24 MS/s for CH1 alone and 12 MS/s with both channels. Roll mode runs the
ADC at most at 400 kS/s, so the scope buffer does not overflow between USB
reads.

## Tests

```
cargo test                    # unit, end-to-end (fake scope) and UI tests
cargo test --test hardware -- --ignored --test-threads=1 --nocapture
```

The hardware tests need the real scope. With a probe on the calibration
output they also check the frequency and the voltage.

## Licence of the firmware

The scope firmware comes from OpenHantek6022 (GPL-3.0). This repository
does not contain it: the program reads the firmware file at run time. The
Nix flake points at the file in the `openhantek6022` source from nixpkgs.
