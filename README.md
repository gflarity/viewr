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

With no argument, `viewr` views the current directory. Given a single image
file instead of a directory, it views that image with the rest of its
directory alongside:

```sh
viewr ~/Pictures/shoot/DSC_0001.NEF
```

## macOS app bundle (Finder)

To open images by double-clicking them in Finder, build the app bundle:

```sh
scripts/make-app.sh            # builds dist/Viewr.app
scripts/make-app.sh --install  # also copies it to /Applications
```

The bundle registers `viewr` as a viewer for JPEG (`.jpg`/`.jpeg`), PNG
(`.png`), and Nikon NEF (`.nef`) files. It is registered as an alternate
handler, so it appears under right-click → Open With without replacing your
current default app. To make it the default for a file type, select a file in
Finder → Get Info → Open with: viewr → Change All…

Opening a file from Finder shows that image with its folder's images
alongside; opening another file while `viewr` is already running moves the
existing window to the new image.

## Keys

| Key | Action |
|---|---|
| `←` / `→` (or `H` / `L`) | Previous / next image (stops at each end) |
| `Home` / `End` | First / last image |
| `+` / `=` , `-` | Zoom in / out; out stops at fit (drag to pan while zoomed) |
| Pinch / spread (trackpad) | Zoom out / in toward the pointer; springs back past fit |
| Two-finger scroll (trackpad) | Pan (`Cmd`-scroll zooms instead) |
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
