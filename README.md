# imora

A lightweight, elegant media gallery written in Rust. Open a folder and browse
its pictures and videos with the arrow keys.

Built with [eframe/egui](https://github.com/emilk/egui) (OpenGL renderer) for
the UI, the `image` crate for stills, and FFmpeg (via `ffmpeg-next`) for video
and audio decoding (played through [cpal](https://crates.io/crates/cpal)).

## Features

- Arrow keys (←/→/↑/↓), PageUp/PageDown, Home/End to browse
- Plays and loops videos in-app **with sound**; `Space` toggles play/pause
  (mutes while paused), drag the progress bar to seek. Audio is resampled to
  your output device; videos without sound, or machines without an audio
  device, simply play silently.
- Slideshow mode (`S` or the `▶` button) auto-advances through images every
  few seconds (type a value in the `⚙` popup or pass `--interval`). Videos
  play to the end — the slideshow moves on only once a video finishes.
- The video progress bar appears when the mouse moves and fades away after a
  moment of inactivity.
- Metadata panel (`I` or the `ⓘ` button): file size, modified date, and
  image/video details (dimensions, duration, codecs, bitrate, container, …)
- Fullscreen shows nothing but the media itself (move the mouse to reveal the video progress bar)
- Filmstrip of lazy-generated thumbnails, hidden by default — press `G` (or the
  `▤` button) to toggle it (click a thumbnail to jump)
- Zoom with the scroll wheel, drag to pan, double-click / `F` for fullscreen
- GIF & animated WebP playback
- Open a folder at launch (`imora <folder>`), via the built-in folder browser
  (`⌂` button / `O` key), or by drag-and-drop
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
| `I`            | Toggle metadata panel                 |
| `O`            | Open a folder (built-in browser)      |
| `E`            | Open current file with system player  |

## Structure

```
src/
  main.rs    CLI parsing + app bootstrap
  app.rs     eframe UI: layout, keys, filmstrip, transitions
  audio.rs   cpal audio output (device format, ring buffer, backpressure)
  browser.rs built-in folder browser (⌂ / O)
  media.rs   folder scanning + image/GIF/WebP decoding
  metadata.rs metadata panel (file size, date, dimensions, codecs, …)
  video.rs   background FFmpeg decode thread (video + audio, play/pause/seek/loop)
  thumbs.rs  lazy thumbnail generation for the filmstrip
```

## Notes

- FFmpeg is expected in the dev shell; if it is unavailable at runtime, videos
  simply report an error in-app while images keep working.
- Audio plays via the ALSA backend of cpal (`alsa-lib` is part of the dev
  shell). On a PipeWire system the ALSA "default" device usually routes to
  PipeWire; if you hear nothing, `cpal` can be switched to its `pulse` feature
  instead.
- AVIF decoding is not enabled yet (needs `libdav1d`); the common still formats
  (JPEG, PNG, WebP, GIF, BMP, TIFF, ICO, EXR, …) are supported.
