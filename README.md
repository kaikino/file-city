# File City

A first-person 3D filesystem explorer written in Rust. It scans a directory
tree and generates a stylized city you can walk around in: directories become
districts, files become buildings and objects you can inspect, read, watch,
listen to and physically throw around.

Built with [Bevy 0.19](https://bevy.org) (rendering via wgpu/Metal — runs
natively on Apple Silicon) and [Avian](https://github.com/avianphysics/avian)
physics.

## What files look like

| File kind | In the city |
| --- | --- |
| Text / code | Obelisks with the file's actual text scrolling up a glowing marquee |
| Images | Buildings with storefront screens showing the actual picture |
| Audio | Pedestals with a pulsing floating orb; play/stop in game |
| Video | Cinema blocks with a glowing marquee bar |
| Archives | Treasure chests with golden lids |
| Executables / libraries | Little robot statues with glowing eyes |
| Data (db, pdf, models, …) | Office towers, height scales with file size |
| Tiny files (<4 KB) | Physics props: crates and balls you can grab, kick and throw |

Districts are laid out with a squarified treemap, separated by roads, walled
at the top level, and labeled with floating signs. Everything is deterministic
for a given folder. The game never writes to the files it shows; the only side
effect is the explicit "open" action.

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
| E | Inspect what you're looking at (read text, view image, play audio) |
| O | Open the file in its default app |
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
| `--shot out.png` | Debug: screenshot shortly after load, then exit | off |

## Notes

- Scanning skips hidden files, symlinks and noisy directories
  (`node_modules`, `target`, `Library`, …) and keeps the largest files per
  directory, so the city stays walkable.
- Text/image screens light up as you approach and are freed when you leave,
  keeping GPU memory bounded on big scans.
