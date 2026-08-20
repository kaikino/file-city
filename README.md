# File City

A first-person 3D filesystem explorer written in Rust. It scans a directory
tree and generates a dense, Tokyo-inspired cyberpunk city you can walk around
in: directories become districts, files become buildings packed
shoulder-to-shoulder along streets and back alleys — with neon signs bearing
their real file names — and everything can be inspected, read, viewed,
listened to and physically thrown around without ever leaving the game.

Built with [Bevy 0.19](https://bevy.org) (rendering via wgpu/Metal — runs
natively on Apple Silicon) and [Avian](https://github.com/avianphysics/avian)
physics.

## The city

- Buildings line the streets in packed rows like a real Japanese shopping
  street, with narrow alleys cutting into district interiors.
- Every building is seeded by its file's path hash: width, height, setback,
  stacked tiers, rooftop water tanks / AC units / antennas, window patterns,
  awnings and neon signage all vary — while height still scales with file
  size.
- Facades are dressed on multiple sides (screens front and back, side
  displays, protruding vertical neon), so the city reads from every angle.
- A physically-based atmosphere runs a full day-night cycle (~8 minutes):
  the sun sweeps the sky, dusk brings the neon up, street lamps switch on,
  windows light, fog thickens and rooftop beacons blink. Drones circle
  overhead and puddles catch the lamp light.

## What files look like

| File kind | In the city |
| --- | --- |
| Text / code | Buildings with the file's actual text scrolling up glowing marquees (front and back) |
| Images | Buildings whose storefront and side screens show the actual picture |
| Audio | Buildings crowned with a pulsing orb on a rooftop pole; play/stop in game |
| Video | Cinema fronts with a wide top screen and marquee bar |
| Archives | Low gold-banded warehouses |
| Executables / libraries | Buildings with robot statues (glowing eyes) on the roof |
| Data (db, pdf, models, …) | The tallest corporate towers, with antennas and beacons |
| Tiny files (<4 KB) | Street props: vending machines, crates and balls you can grab, kick and throw |

Everything is deterministic for a given folder. The game never writes to or
executes the files it shows.

## Everything opens in-game

Files are never launched or opened by an external app:

- **Text / code** — fullscreen reader with scrolling.
- **Images** — fullscreen viewer.
- **Audio** — decoded and played by the engine (mp3/wav/flac/ogg).
- **Archives** — zip/jar/whl/tar/tgz/crate contents listed in the reader.
- **Executables, data, unknown binaries** — classic hex+ASCII dump viewer.
- **Video** — metadata card (video decoding is the one thing not done
  in-game).

The one deliberate bridge back to the OS: press **R** to reveal a file's
location in Finder — it selects the file in its folder without opening it.

## Run

```bash
# Scan your home directory (default)
cargo run --release

# Scan a specific folder
cargo run --release -- ~/Documents

# Options
cargo run --release -- ~/code --depth 4 --max-files 2500
```

First build takes a few minutes (Bevy). For faster iteration builds during
development: `cargo run --features dev`.

## Controls

| Input | Action |
| --- | --- |
| Click | Capture mouse / shoot projectile / throw carried prop |
| WASD | Move |
| Shift | Sprint |
| Space | Jump |
| E | Inspect: read text, view image, play audio, list archive, hex dump |
| R | Reveal the file's location in Finder (does not open the file) |
| F | Grab / drop a small prop (gravity-gun carry) |
| Esc | Close overlay / release mouse |

The HUD shows the district (directory) you are standing in at the top left,
and details of whatever the crosshair points at.

## Flags

| Flag | Meaning | Default |
| --- | --- | --- |
| `<path>` | Root directory to scan | `$HOME` |
| `--depth N` | Max directory depth | 3 |
| `--max-files N` | Global file cap | 1600 |
| `--tod T` | Start time of day, 0..1 (0 = midnight, 0.5 = noon) | 0.77 (dusk) |
| `--shot out.png` | Debug: screenshot shortly after load, then exit | off |

## Notes

- Scanning skips hidden files, symlinks and noisy directories
  (`node_modules`, `target`, `Library`, …) and keeps the largest files per
  directory, so the city stays walkable.
- Text/image screens and neon name signs light up as you approach and are
  freed when you leave, keeping GPU memory bounded on big scans.
