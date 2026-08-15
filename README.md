# imora

A lightweight, elegant media gallery written in Rust. Open a folder and browse
its pictures and videos with the arrow keys.

Built with [eframe/egui](https://github.com/emilk/egui) (OpenGL renderer) for
the UI, the `image` crate for stills, and FFmpeg (via `ffmpeg-next`) for video
decoding. No audio yet — videos play visually, in-app.

## Features

- Arrow keys (←/→/↑/↓), PageUp/PageDown, Home/End to browse
- Plays and loops videos in-app; `Space` toggles play/pause, drag the progress bar to seek
- Slideshow mode (`S` or the `▶` button) auto-advances through images every
  few seconds (type a value in the `⚙` popup or pass `--interval`). Videos
  play to the end — the slideshow moves on only once a video finishes.
- The video progress bar appears when the mouse moves and fades away after a
  moment of inactivity.
- Fullscreen shows nothing but the media itself (move the mouse to reveal the video progress bar)
- Filmstrip of lazy-generated thumbnails (click to jump, `G` to toggle)
- Zoom with the scroll wheel, drag to pan, double-click / `F` for fullscreen
- GIF & animated WebP playback
- Open a folder at launch (`imora <folder>`), via the `⌂` button, or drag-and-drop
- `E` opens the current file with your system player (via `xdg-open`)
- By default the window has no decorations — imora follows systems that run
  with window decorations disabled. Pass `--decorations` to opt back in.

## Development

The dev environment is a Nix shell. Both entry points provide the identical,
pinned environment (Rust toolchain, FFmpeg 8.1, OpenGL/X11/Wayland libs):

```sh
nix develop          # flake (recommended)
nix-shell shell.nix  # classic alternative
```

Then build and run:

```sh
cargo run --release -- /path/to/your/media
```

Useful commands:

```sh
cargo build            # debug build
cargo build --release  # optimized build
cargo clippy           # lints
cargo run --release -- --help
```

## Usage

```
USAGE:
    imora [OPTIONS] [FOLDER]

OPTIONS:
    -d, --decorations    Show window decorations (minimize/maximize/close).
                         Default: none — imora follows systems that run with
                         window decorations disabled.
    -f, --fullscreen     Start in fullscreen
    -i, --interval <SECONDS>
                         Slideshow interval between items (default: 2.0)
    -s, --slideshow      Start the slideshow automatically
    -h, --help           Print this help
    -v, --version        Print version
```

## Layout

| Key            | Action                                |
| -------------- | ------------------------------------- |
| `←` `→` `↑` `↓` | Previous / next media                 |
| `PageUp/Down`  | Jump 10 items                         |
| `Home` / `End` | First / last item                     |
| `Space`        | Play / pause video                 |
| `S`            | Toggle slideshow                   |
| `+` / `-` / `0` | Zoom in / out / reset            |
| scroll wheel   | Zoom (over the media area)            |
| drag           | Pan when zoomed                       |
| `F` / dbl-click| Toggle fullscreen                    |
| `G`            | Toggle filmstrip                      |
| `O`            | Open a folder                         |
| `E`            | Open current file with system player  |

## Structure

```
src/
  main.rs    CLI parsing + app bootstrap
  app.rs     eframe UI: layout, keys, filmstrip, transitions
  media.rs   folder scanning + image/GIF/WebP decoding
  video.rs   background FFmpeg decode thread (play/pause/seek/loop)
  thumbs.rs  lazy thumbnail generation for the filmstrip
```

## Notes

- FFmpeg is expected in the dev shell; if it is unavailable at runtime, videos
  simply report an error in-app while images keep working.
- AVIF decoding is not enabled yet (needs `libdav1d`); the common still formats
  (JPEG, PNG, WebP, GIF, BMP, TIFF, ICO, EXR, …) are supported.
