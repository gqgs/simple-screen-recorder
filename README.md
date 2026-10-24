# Simple screen recorder

A small Rust CLI for Linux desktops with an XDG ScreenCast portal (including KDE
Wayland and GNOME). Select a window or monitor in the desktop picker, or capture a
rectangle within the selected monitor. Stop with **Ctrl+C** to export an MP4.

## Install

Requires Rust, FFmpeg with libx264/AAC, GStreamer development libraries, the
GStreamer PipeWire plugin, and a running desktop portal backend.

Arch Linux / KDE:

```sh
sudo pacman -S --needed rust pkgconf ffmpeg gstreamer gst-plugins-base \
  gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-plugin-pipewire \
  xdg-desktop-portal xdg-desktop-portal-kde
cargo build --release
./target/release/ssr doctor
```

Debian / Ubuntu (use your desktop's portal backend):

```sh
sudo apt install build-essential pkg-config libgstreamer1.0-dev \
  ffmpeg gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-pipewire \
  xdg-desktop-portal xdg-desktop-portal-gtk
# KDE: install xdg-desktop-portal-kde; GNOME: xdg-desktop-portal-gnome.
# Install a current stable Rust toolchain, then:
cargo build --release
```

Use `./target/release/ssr`, or install the binary onto your PATH with
`cargo install --path . --locked`.

## Record

```sh
# Pick a window
ssr record --window -o window.mp4

# Pick a monitor, then capture a rectangle within it
ssr record --region 100,200,1280,720 -o rectangle.mp4

# Pick an entire monitor, record for 15 seconds
ssr record --duration 15 -o desktop.mp4

# 30 fps variant with system audio
ssr record --window --preset twitter-30 --audio @DEFAULT_MONITOR@ -o demo.mp4

# Microphone instead of system audio
ssr record --window --audio @DEFAULT_SOURCE@ -o narrated.mp4
```

Rectangle coordinates are **physical pixels relative to the selected stream's
top-left**, not global desktop or logical/scaled coordinates. They must fit inside
the stream; invalid bounds fail capture. Rectangle selection is numeric, not a
mouse-drag overlay. Window selection uses the desktop portal, so it works with
native Wayland windows without relying on X11 window IDs. Availability and behavior
when a window is minimized or resized depend on the compositor.

Audio is off by default. `--audio` accepts a PulseAudio/PipeWire-Pulse source;
`pactl list short sources` lists names. This records one audio source, not a mix of
microphone and system audio. Use `--hide-cursor` to omit the pointer.

Existing outputs are protected unless `--overwrite` is supplied. Exports are
written to a staging file beside the destination and only published on success.
Capture uses a lossless H.264/FLAC Matroska intermediate beside the output; allow
extra disk space. The slower final encode starts after recording stops. Failed
recordings/exports retain the intermediate for recovery:

```sh
ssr export .ssr-capture-XXXXXX.mkv --preset twitter -o recovered.mp4
```

A portal permission dialog is expected for each recording. There is no direct X11,
macOS, or Windows backend. `doctor` checks local plugins and encoders; it does not
open a portal dialog or validate audio devices.

## Presets

```sh
ssr presets
ssr record --window --preset presets/twitter.toml -o demo.mp4
ssr export input.mkv --preset twitter-30 -o smaller.mp4
```

| Setting | `twitter` | `twitter-30` |
| --- | --- | --- |
| Container / video | MP4 / H.264 (libx264) | same |
| Maximum size | 1920 × 1080 | same |
| Frame rate | 60 fps | 30 fps |
| Quality / encoder | CRF 18 / slow | same |
| VBV maxrate / buffer | 8 Mbps / 16 Mbit | same |
| Pixel format | yuv420p | same |
| Audio (when present) | AAC 160 kbps | same |

These settings follow the supplied Twitter reference. Dimensions preserve aspect
ratio, never upscale, and round down to even pixels for H.264 compatibility; a
small rectangle stays small. No padding is added. The bitrate ceiling is x264's
VBV rate constraint, not a guarantee about every instantaneous packet or total
file size. This tool does not enforce upload duration or file-size limits.

Copy `presets/twitter.toml` to define another export configuration. All fields are
required; unknown fields and invalid values are rejected. The minimal preset
format keeps MP4/H.264/AAC fixed and lets you configure dimensions, frame rate,
CRF, bitrate limits, encoder speed, and audio bitrate.

Capture integration uses [ashpd's ScreenCast portal API](https://docs.rs/ashpd/0.13.13/ashpd/desktop/screencast/index.html)
and GStreamer's PipeWire source.

## Development

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
# Synthetic capture/export checks; requires GStreamer plugins, ffmpeg, ffprobe.
# Does not open the desktop picker or capture real screen content.
cargo test --test media -- --ignored
```
