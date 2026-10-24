mod preset;

use anyhow::{Context, Result, bail};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use clap::{Parser, Subcommand};
use gstreamer::{self as gst, prelude::*};
use preset::Preset;
use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "ssr",
    version,
    about = "Minimal Linux screen recorder (Wayland / desktop portal)"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Record a monitor, rectangle, or window. Ctrl+C stops and exports.
    Record {
        #[arg(short, long, default_value = "recording.mp4")]
        output: PathBuf,
        /// Open the desktop's window picker instead of its monitor picker
        #[arg(long, conflicts_with = "region")]
        window: bool,
        /// Rectangle X,Y,WIDTH,HEIGHT in pixels relative to the selected monitor
        #[arg(long)]
        region: Option<Region>,
        /// Built-in name (twitter, twitter-30) or path to a TOML preset
        #[arg(short, long, default_value = "twitter")]
        preset: String,
        /// PulseAudio/PipeWire-Pulse source (e.g. @DEFAULT_MONITOR@ or @DEFAULT_SOURCE@)
        #[arg(long)]
        audio: Option<String>,
        /// Stop automatically after this many seconds
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        duration: Option<u64>,
        /// Discard this many initial seconds while the desktop picker disappears
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(0..=60))]
        startup_delay: u64,
        /// Do not embed the mouse pointer
        #[arg(long)]
        hide_cursor: bool,
        /// Replace an existing output file after a successful export
        #[arg(long)]
        overwrite: bool,
        /// Use a test pattern instead of opening the desktop picker
        #[arg(long, hide = true, conflicts_with = "window")]
        test_source: bool,
    },
    /// Export an existing recording using a preset
    Export {
        input: PathBuf,
        /// Trim this many seconds from the beginning (also trims audio)
        #[arg(long, default_value_t = 0)]
        trim_start: u64,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short, long, default_value = "twitter")]
        preset: String,
        #[arg(long)]
        overwrite: bool,
    },
    /// List built-in export presets
    Presets,
    /// Check capture plugins and FFmpeg availability
    Doctor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Region {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}
impl FromStr for Region {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let values: Vec<i32> = s
            .split(',')
            .map(|v| v.trim().parse())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| "Use X,Y,WIDTH,HEIGHT (nonnegative pixel coordinates)")?;
        if values.len() != 4 {
            return Err("Use X,Y,WIDTH,HEIGHT".into());
        }
        let r = Self {
            x: values[0],
            y: values[1],
            width: values[2],
            height: values[3],
        };
        if r.x < 0
            || r.y < 0
            || r.width < 2
            || r.height < 2
            || r.x.checked_add(r.width).is_none()
            || r.y.checked_add(r.height).is_none()
        {
            return Err("Coordinates must be nonnegative and dimensions at least 2 pixels, without overflow".into());
        }
        Ok(r)
    }
}

fn check_plugins(test: bool, audio: bool) -> Result<()> {
    gst::init()?;
    let mut names = vec![
        "queue",
        "videoconvert",
        "videorate",
        "videocrop",
        "x264enc",
        "h264parse",
        "matroskamux",
        "filesink",
    ];
    names.push(if test { "videotestsrc" } else { "pipewiresrc" });
    if audio {
        names.extend(["pulsesrc", "audioconvert", "audioresample", "flacenc"]);
    }
    let missing: Vec<_> = names
        .into_iter()
        .filter(|n| gst::ElementFactory::find(n).is_none())
        .collect();
    if !missing.is_empty() {
        bail!(
            "Missing GStreamer plugins: {}. See README.md for installation packages.",
            missing.join(", ")
        );
    }
    Ok(())
}

fn check_ffmpeg() -> Result<()> {
    let output = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .context("Install FFmpeg with libx264 and AAC support")?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !text.contains("libx264") || !text.contains(" aac ") {
        bail!("FFmpeg must provide libx264 and AAC encoders");
    }
    Ok(())
}

fn check_output(output: &Path, overwrite: bool) -> Result<()> {
    if output.exists() && !overwrite {
        bail!("{} already exists; use --overwrite", output.display());
    }
    if output.is_dir() {
        bail!("Output is a directory");
    }
    Ok(())
}

fn export(p: &Preset, input: &Path, output: &Path, overwrite: bool, trim_start: u64) -> Result<()> {
    check_output(output, overwrite)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staged = tempfile::Builder::new()
        .prefix(".ssr-export-")
        .suffix(".mp4")
        .tempfile_in(parent)?;
    p.export(input, staged.path(), trim_start)?;
    if overwrite {
        staged.persist(output)?;
    } else {
        staged.persist_noclobber(output)?;
    }
    eprintln!("Saved {}", output.display());
    Ok(())
}

struct RunningPipeline(gst::Pipeline);
impl Drop for RunningPipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

async fn capture(pipeline: gst::Pipeline, duration: Option<u64>, startup_delay: u64) -> Result<()> {
    let running = RunningPipeline(pipeline);
    let pipeline = &running.0;
    let bus = pipeline.bus().context("No GStreamer bus")?;
    // Register the signal handler before starting capture.
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    pipeline.set_state(gst::State::Playing)?;
    if startup_delay > 0 {
        eprintln!(
            "Waiting {startup_delay}s for the picker to disappear (startup frames will be discarded)…"
        );
    }
    let mut announced = false;
    let start = Instant::now();
    let mut stopping: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        if !announced && start.elapsed() >= Duration::from_secs(startup_delay) {
            eprintln!("Recording. Press Ctrl+C to stop and export.");
            announced = true;
        }
        tokio::select! {
            _ = interrupt.recv() => {
                if stopping.is_none() {
                    pipeline.send_event(gst::event::Eos::new());
                    stopping = Some(Instant::now());
                    eprintln!("Finalizing capture…");
                }
            }
            _ = tick.tick() => {}
        }
        if stopping.is_none()
            && duration.is_some_and(|d| {
                start
                    .elapsed()
                    .saturating_sub(Duration::from_secs(startup_delay))
                    >= Duration::from_secs(d)
            })
        {
            pipeline.send_event(gst::event::Eos::new());
            stopping = Some(Instant::now());
        }
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Eos(..) => {
                    if !announced {
                        bail!("Stopped during startup; no recording to export");
                    }
                    return Ok(());
                }
                gst::MessageView::Error(err) => {
                    bail!("Capture failed: {} ({:?})", err.error(), err.debug())
                }
                _ => {}
            }
        }
        if stopping.is_some_and(|t| t.elapsed() > Duration::from_secs(20)) {
            bail!("Timed out finalizing capture");
        }
    }
}

fn pipeline(
    source: &str,
    region: Option<Region>,
    fps: u32,
    audio: Option<&str>,
    path: &Path,
) -> Result<gst::Pipeline> {
    let crop = region
        .map(|r| {
            format!(
                "videocrop left={} top={} right=-1 bottom=-1 ! video/x-raw,width={},height={} ! ",
                r.x, r.y, r.width, r.height
            )
        })
        .unwrap_or_default();
    let audio_branch = if audio.is_some() {
        "pulsesrc name=audio_source do-timestamp=true ! queue ! audioconvert ! audioresample ! flacenc ! queue ! mux."
    } else {
        ""
    };
    let description = format!(
        "{source} ! queue ! videoconvert ! {crop}videorate ! video/x-raw,format=Y444,framerate={fps}/1 ! x264enc speed-preset=ultrafast tune=zerolatency pass=quant quantizer=0 ! h264parse ! queue ! mux. matroskamux name=mux ! filesink name=output {audio_branch}"
    );
    let pipeline = gst::parse::launch(&description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("Invalid capture pipeline"))?;
    pipeline
        .by_name("output")
        .context("Missing output element")?
        .set_property(
            "location",
            path.to_str().context("Output path must be UTF-8")?,
        );
    if let Some(device) = audio {
        pipeline
            .by_name("audio_source")
            .context("Missing audio element")?
            .set_property("device", device);
    }
    Ok(pipeline)
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Action::Presets => println!(
            "twitter     MP4 / H.264 / up to 1920×1080 / 60 fps / CRF 18 / 8 Mbps / slow / AAC 160k\ntwitter-30  Same settings at 30 fps\nCustom:     --preset path/to/preset.toml"
        ),
        Action::Doctor => {
            check_ffmpeg()?;
            check_plugins(false, true)?;
            println!(
                "FFmpeg and capture plugins are available. A working ScreenCast portal is required at recording time."
            );
        }
        Action::Export {
            input,
            trim_start,
            output,
            preset,
            overwrite,
        } => {
            let p = Preset::load(&preset)?;
            check_ffmpeg()?;
            export(&p, &input, &output, overwrite, trim_start)?;
        }
        Action::Record {
            output,
            window,
            region,
            preset,
            audio,
            duration,
            hide_cursor,
            startup_delay,
            overwrite,
            test_source,
        } => {
            let p = Preset::load(&preset)?;
            check_output(&output, overwrite)?;
            check_ffmpeg()?;
            check_plugins(test_source, audio.is_some())?;
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            // Keep the intermediate beside the output so failures can be recovered.
            let (file, intermediate) = tempfile::Builder::new()
                .prefix(".ssr-capture-")
                .suffix(".mkv")
                .tempfile_in(parent)?
                .keep()?;
            drop(file);
            eprintln!("Intermediate: {}", intermediate.display());
            let result: Result<()> = async {
                if test_source {
                    capture(
                        pipeline(
                            "videotestsrc is-live=true ! video/x-raw,width=640,height=360",
                            region,
                            p.fps,
                            audio.as_deref(),
                            &intermediate,
                        )?,
                        duration,
                        startup_delay,
                    )
                    .await?;
                } else {
                    let portal = Screencast::new().await?;
                    let session = portal.create_session(Default::default()).await?;
                    let options = SelectSourcesOptions::default()
                        .set_sources(Some(
                            if window {
                                SourceType::Window
                            } else {
                                SourceType::Monitor
                            }
                            .into(),
                        ))
                        .set_multiple(false)
                        .set_cursor_mode(if hide_cursor {
                            CursorMode::Hidden
                        } else {
                            CursorMode::Embedded
                        });
                    portal.select_sources(&session, options).await?.response()?;
                    let response = portal
                        .start(&session, None, Default::default())
                        .await?
                        .response()?;
                    let stream = response.streams().first().context("No source selected")?;
                    let remote = portal
                        .open_pipe_wire_remote(&session, Default::default())
                        .await?;
                    let source = format!(
                        "pipewiresrc fd={} path={} do-timestamp=true",
                        remote.as_raw_fd(),
                        stream.pipe_wire_node_id()
                    );
                    let result = capture(
                        pipeline(&source, region, p.fps, audio.as_deref(), &intermediate)?,
                        duration,
                        startup_delay,
                    )
                    .await;
                    let _ = session.close().await;
                    result?;
                }
                eprintln!("Exporting with preset {preset}…");
                export(&p, &intermediate, &output, overwrite, startup_delay)?;
                Ok(())
            }
            .await;
            if let Err(err) = result {
                return Err(err.context(format!(
                    "Intermediate retained at {} (if capture started). Retry with ssr export.",
                    intermediate.display()
                )));
            }
            std::fs::remove_file(intermediate)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rectangles_validate_bounds_and_overflow() {
        assert_eq!(
            "10,20,800,600".parse::<Region>().unwrap(),
            Region {
                x: 10,
                y: 20,
                width: 800,
                height: 600
            }
        );
        for bad in [
            "",
            "1,2,3",
            "-1,0,20,20",
            "0,0,0,20",
            "2147483647,0,20,20",
            "a,0,20,20",
        ] {
            assert!(bad.parse::<Region>().is_err(), "{bad}");
        }
    }
    #[test]
    fn selection_modes_are_exclusive() {
        assert!(
            Cli::try_parse_from(["ssr", "record", "--window", "--region", "0,0,100,100"]).is_err()
        );
    }
}
