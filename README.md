# viewr

A minimalist native image viewer for macOS. Point it at a directory of
photos and flip through them with the arrow keys. JPEG, PNG, and Nikon
NEF files (via their embedded JPEG previews) are supported.

## Build and install

Prerequisite: a Rust toolchain (`rustc --version` should work; install
via [rustup](https://rustup.rs) if not).

```sh
# run straight from the source tree
cargo run --release -- ~/Pictures/shoot

# or install into ~/.cargo/bin so `viewr` works anywhere
cargo install --path .
viewr ~/Pictures/shoot
```

With no directory argument, `viewr` views the current directory.

## Keys

| Key | Action |
|---|---|
| `←` / `→` (or `H` / `L`) | Previous / next image (wraps around) |
| `Home` / `End` | First / last image |
| `+` / `=` , `-` | Zoom in / out; out stops at fit (drag to pan while zoomed) |
| Pinch / spread (trackpad) | Zoom out / in toward the pointer; springs back past fit |
| `0` | Fit to window |
| `1` | Actual size (100%) |
| `F` | Toggle fullscreen |
| `Q` | Quit |
| `Esc` | Exit fullscreen (or quit when not fullscreen) |

The window title shows the file name, dimensions, and zoom level.

## Key hook (extensions)

`viewr` can notify an external program of every keypress via a subprocess
hook:

```sh
viewr --on-key ./my-hook.sh ~/Pictures/shoot
```

On each handled key, `viewr` spawns `my-hook.sh <file> <key>` (detached,
fire-and-forget), where `<file>` is the image on screen and `<key>` is the
key name (e.g. `ArrowRight`). Any executable works — shell, Python, or a
compiled binary.

## Formats

- JPEG (`.jpg`, `.jpeg`) and PNG (`.png`), any case; EXIF orientation is
  applied automatically.
- Nikon NEF (`.nef`): the embedded JPEG preview is extracted and shown.
  This matches what the camera's own rendering looks like, without the
  cost of developing the raw.

## Known limitations

- Embedded NEF previews are displayed as sRGB; a preview shot in Adobe RGB
  will look slightly desaturated.
- NEF files whose preview is not a plain JPEG (e.g. newer high-efficiency
  NEFs) are reported as unloadable rather than decoded.
- macOS only for now; the `eframe` UI layer keeps a future Linux/Windows
  port straightforward.

## License

Apache License, Version 2.0 — see `LICENSE-APACHE`.
Parts of `src/tiff` and `src/nef.rs` are vendored from LightCraft; see
`ATTRIBUTION.md`.
