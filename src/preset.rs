use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{path::Path, process::Command};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub maxrate_kbps: u32,
    pub bufsize_kbps: u32,
    pub encoder_preset: String,
    pub audio_kbps: u32,
}

impl Preset {
    pub fn load(name: &str) -> Result<Self> {
        let text = match name {
            "twitter" => include_str!("../presets/twitter.toml").to_owned(),
            "twitter-30" => include_str!("../presets/twitter-30.toml").to_owned(),
            path => std::fs::read_to_string(path).context("Cannot read preset file")?,
        };
        let p: Self = toml::from_str(&text).context("Invalid preset TOML")?;
        if p.width < 2
            || p.height < 2
            || p.width > 16384
            || p.height > 16384
            || !p.width.is_multiple_of(2)
            || !p.height.is_multiple_of(2)
            || !(1..=60).contains(&p.fps)
            || p.crf > 51
            || p.maxrate_kbps == 0
            || p.bufsize_kbps == 0
            || !(16..=512).contains(&p.audio_kbps)
        {
            bail!(
                "Preset needs even dimensions (2–16384), FPS 1–60, CRF 0–51, positive bitrate limits and audio 16–512 kbps"
            );
        }
        if ![
            "ultrafast",
            "superfast",
            "veryfast",
            "faster",
            "fast",
            "medium",
            "slow",
            "slower",
            "veryslow",
        ]
        .contains(&p.encoder_preset.as_str())
        {
            bail!("Unknown x264 encoder preset");
        }
        Ok(p)
    }

    pub fn export(&self, input: &Path, output: &Path, trim_start: u64) -> Result<()> {
        let filter = format!(
            "scale=w='min(iw,{})':h='min(ih,{})':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,fps={}",
            self.width, self.height, self.fps
        );
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(input)
            // Output-side seeking decodes/discards startup frames accurately,
            // including non-keyframes, and shifts both audio and video together.
            .args(["-ss", &trim_start.to_string()])
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                "-vf",
                &filter,
                "-c:v",
                "libx264",
                "-preset",
                &self.encoder_preset,
                "-crf",
                &self.crf.to_string(),
                "-maxrate",
                &format!("{}k", self.maxrate_kbps),
                "-bufsize",
                &format!("{}k", self.bufsize_kbps),
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                &format!("{}k", self.audio_kbps),
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ])
            .arg(output)
            .status()
            .context("Cannot run ffmpeg; install FFmpeg with libx264")?;
        if !status.success() {
            bail!("FFmpeg export failed ({status})");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_match_requested_settings() {
        let p = Preset::load("twitter").unwrap();
        assert_eq!(
            (
                p.width,
                p.height,
                p.fps,
                p.crf,
                p.maxrate_kbps,
                p.audio_kbps
            ),
            (1920, 1080, 60, 18, 8000, 160)
        );
        assert_eq!(Preset::load("twitter-30").unwrap().fps, 30);
    }
}
