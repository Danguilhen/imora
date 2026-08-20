# imora

A lightweight, elegant media gallery written in Rust. Open a folder and browse
its pictures and videos with the arrow keys — or pipe in a URL like mpv.

Built with [eframe/egui](https://github.com/emilk/egui) (OpenGL renderer) for
the UI, the `image` crate for stills, and FFmpeg (via `ffmpeg-next`) for video
and audio decoding (played through [cpal](https://crates.io/crates/cpal)).

## Features

- Arrow keys (←/→/↑/↓), PageUp/PageDown, Home/End to browse
- **Network streams**: pass a URL (`https://`, `rtsp://`, …) instead of a
  folder, or just paste one anywhere with `Ctrl+V`. Direct links play as-is;
  pages from streaming sites (YouTube etc.) are resolved with
  [yt-dlp](https://github.com/yt-dlp/yt-dlp), like mpv's `ytdl` hook.
- Plays and loops videos in-app **with sound**; `Space` toggles play/pause
  (mutes while paused), drag the progress bar to seek. Audio is resampled to
  your output device; videos without sound, or machines without an audio
  device, simply play silently. Audio never stalls the video, so playback
  stays smooth.
- Videos with several audio/video/subtitle streams get a **tracks** button in
  the toolbar to switch between them (e.g. different languages or angles);
  selected subtitles are rendered over the video and disappear at their end
  time. Initial tracks can be chosen at launch with `--audio-track` /
  `--subtitle-track <index>`.
- The video controls include a play/pause button and a seek bar you can click
  or drag to jump to any time.
- Slideshow mode (`S` or the `▶` button) auto-advances through images every
  few seconds (type a value in the `⚙` popup or pass `--interval`). Videos
  play to the end — the slideshow moves on only once a video finishes.
- Playback settings (`⚙` popup): toggle video looping, include/exclude videos
  from the rotation, shuffle (random) order, and crossfade transitions with a
  configurable duration (typed in seconds).
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
  (`⌂` button / `O` key — arrows navigate, `Enter` opens the selection), by
  drag-and-drop onto the window, or by pasting its path
- Grab and move the window itself by dragging empty canvas — there is no
  title bar by default
- `E` opens the current file with your system player (via `xdg-open`)
- By default the window has no decorations — imora follows systems that run
  with window decorations disabled. Pass `--decorations` to opt back in.

## Quick start

The easiest way is [Nix](https://nixos.org). From a clone of this repository:

```sh
nix run . -- ~/Pictures            # run without installing
nix profile install .              # or install it permanently
```

The flake ships everything imora needs at runtime, including `yt-dlp` for
streaming-site URLs.

Already have a Rust toolchain? You can also use plain cargo, but you need the
native libraries first (FFmpeg 8, ALSA, OpenGL, Wayland/X11, fontconfig;
see `flake.nix` for the exact list):

```sh
cargo run --release -- ~/Pictures
```

## Development

The dev environment is a Nix shell. Both entry points provide the identical,
pinned environment (Rust toolchain, FFmpeg 8.1, yt-dlp, OpenGL/X11/Wayland
libs):

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
cargo test             # unit tests
cargo run --release -- --help
```

## Usage

```
USAGE:
    imora [OPTIONS] [FOLDER]

ARGS:
    <FOLDER>    Folder to open at launch; a network stream URL
                (https://, rtsp://, …) plays it directly

OPTIONS:
    -d, --decorations   Show window decorations (minimize/maximize/close).
                        Default: none — imora follows systems that run with
                        window decorations disabled.
    -f, --fullscreen    Start in fullscreen
    -i, --interval <SECONDS>
                        Slideshow interval between items (default: 3.0)
    -s, --slideshow     Start the slideshow automatically
        --audio-track <N>
                        Initial audio track (stream index; see ⓘ metadata)
        --subtitle-track <N>
                        Initial subtitle track (stream index)
    -h, --help          Print this help
    -v, --version       Print version

NOTES:
    Paste a network stream URL (Ctrl+V) to play it directly, like mpv.
    URLs from streaming sites (YouTube etc.) need yt-dlp installed.
```

## Layout

| Key             | Action                                        |
| --------------- | --------------------------------------------- |
| `←` `→` `↑` `↓` | Previous / next media                         |
| `PageUp/Down`   | Jump 10 items                                 |
| `Home` / `End`  | First / last item                             |
| `Space`         | Play / pause video                            |
| `S`             | Toggle slideshow                              |
| `+` / `-` / `0` | Zoom in / out / reset                         |
| scroll wheel    | Zoom (over the media area)                    |
| drag            | Move the window · pan the image when zoomed   |
| `Ctrl+V`        | Play a pasted network stream URL              |
| `F` / dbl-click | Toggle fullscreen                             |
| `G`             | Toggle filmstrip                              |
| `I`             | Toggle metadata panel                         |
| `O`             | Open a folder (built-in browser; `Enter` confirms) |
| `E`             | Open current file with system player          |

## Structure

```
src/
  main.rs    CLI parsing + app bootstrap
  app.rs     eframe UI: layout, keys, filmstrip, transitions, paste/drag handling
  audio.rs   cpal audio output (device format, ring buffer, backpressure)
  browser.rs built-in folder browser (⌂ / O)
  media.rs   folder scanning, image/GIF/WebP decoding, stream-URL resolution
  metadata.rs metadata panel (file size, date, dimensions, codecs, …)
  video.rs   background FFmpeg decode thread (video + audio, play/pause/seek/loop)
  thumbs.rs  lazy thumbnail generation for the filmstrip
```

## Notes

- FFmpeg is expected in the dev shell; if it is unavailable at runtime, videos
  simply report an error in-app while images keep working.
- Streaming-site URLs need [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) (or
  `youtube-dl`) on `PATH`. It is included in the dev shell and bundled
  automatically when imora is installed through this flake; direct media URLs
  play without it.
- Audio plays via the ALSA backend of cpal (`alsa-lib` is part of the dev
  shell). On a PipeWire system the ALSA "default" device usually routes to
  PipeWire; if you hear nothing, `cpal` can be switched to its `pulse` feature
  instead.
- AVIF decoding is not enabled yet (needs `libdav1d`); the common still formats
  (JPEG, PNG, WebP, GIF, BMP, TIFF, ICO, EXR, …) are supported.
