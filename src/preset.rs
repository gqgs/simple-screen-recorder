use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{path::Path, process::Command};

// Untagged keeps existing sharing preset files backward compatible. Each variant
// rejects unknown fields, so lossless settings cannot silently ignore scaling/VBV.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Preset {
    Lossless(LosslessPreset),
    Sharing(SharingPreset),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LosslessPreset {
    #[serde(rename = "mode")]
    _mode: LosslessMode,
    pub fps: u32,
    pub encoder_preset: String,
    pub audio_kbps: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LosslessMode {
    Lossless,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharingPreset {
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
            "lossless" => include_str!("../presets/lossless.toml").to_owned(),
            "twitter" => include_str!("../presets/twitter.toml").to_owned(),
            "twitter-30" => include_str!("../presets/twitter-30.toml").to_owned(),
            path => std::fs::read_to_string(path).context("Cannot read preset file")?,
        };
        let p: Self = toml::from_str(&text).context("Invalid preset TOML")?;
        p.validate()?;
        Ok(p)
    }

    pub fn fps(&self) -> u32 {
        match self {
            Self::Lossless(p) => p.fps,
            Self::Sharing(p) => p.fps,
        }
    }

    fn encoder_preset(&self) -> &str {
        match self {
            Self::Lossless(p) => &p.encoder_preset,
            Self::Sharing(p) => &p.encoder_preset,
        }
    }

    fn audio_kbps(&self) -> u32 {
        match self {
            Self::Lossless(p) => p.audio_kbps,
            Self::Sharing(p) => p.audio_kbps,
        }
    }

    fn validate(&self) -> Result<()> {
        if !(1..=60).contains(&self.fps()) || !(16..=512).contains(&self.audio_kbps()) {
            bail!("Preset needs FPS 1–60 and audio 16–512 kbps");
        }
        if let Self::Sharing(p) = self
            && (p.width < 2
                || p.height < 2
                || p.width > 16384
                || p.height > 16384
                || !p.width.is_multiple_of(2)
                || !p.height.is_multiple_of(2)
                || p.crf > 51
                || p.maxrate_kbps == 0
                || p.bufsize_kbps == 0)
        {
            bail!(
                "Sharing preset needs even dimensions (2–16384), CRF 0–51 and positive bitrate limits"
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
        .contains(&self.encoder_preset())
        {
            bail!("Unknown x264 encoder preset");
        }
        Ok(())
    }

    pub fn export(&self, input: &Path, output: &Path, trim_start: u64) -> Result<()> {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(input)
            // Output-side seeking discards startup frames accurately, including
            // non-keyframes, and shifts video and audio together.
            .args(["-ss", &trim_start.to_string()])
            .args(["-map", "0:v:0", "-map", "0:a?", "-c:v", "libx264"])
            .args(["-preset", self.encoder_preset()]);
        match self {
            Self::Lossless(_) => {
                // Preserve captured 8-bit Y444 samples, dimensions and cadence.
                // '+' rejects other input formats instead of silently converting
                // color or bit depth. x264 lossless must not be constrained by VBV.
                command.args([
                    "-crf",
                    "0",
                    "-pix_fmt",
                    "+yuv444p",
                    "-fps_mode",
                    "passthrough",
                    "-enc_time_base:v",
                    "-1",
                ]);
            }
            Self::Sharing(p) => {
                let filter = format!(
                    "scale=w='min(iw,{})':h='min(ih,{})':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,fps={}",
                    p.width, p.height, p.fps
                );
                command.args([
                    "-vf",
                    &filter,
                    "-crf",
                    &p.crf.to_string(),
                    "-maxrate",
                    &format!("{}k", p.maxrate_kbps),
                    "-bufsize",
                    &format!("{}k", p.bufsize_kbps),
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
        }
        let status = command
            .args([
                "-c:a",
                "aac",
                "-b:a",
                &format!("{}k", self.audio_kbps()),
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
        let Preset::Sharing(p) = Preset::load("twitter").unwrap() else {
            panic!("sharing preset");
        };
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
        assert_eq!(Preset::load("twitter-30").unwrap().fps(), 30);
    }
    #[test]
    fn lossless_is_explicit_and_rejects_conflicting_settings() {
        let p = Preset::load("lossless").unwrap();
        assert!(matches!(p, Preset::Lossless(_)));
        assert_eq!(p.fps(), 60);
        let source = include_str!("../presets/lossless.toml");
        for extra in [
            "width = 1920",
            "crf = 18",
            "maxrate_kbps = 8000",
            "pixel_format = 'yuv420p'",
        ] {
            assert!(toml::from_str::<Preset>(&format!("{source}\n{extra}")).is_err());
        }
        assert!(toml::from_str::<Preset>(&source.replace("lossless\"", "unknown\"")).is_err());
        for (from, to) in [
            ("fps = 60", "fps = 0"),
            ("fps = 60", "fps = 61"),
            ("audio_kbps = 160", "audio_kbps = 0"),
            ("slow", "invalid"),
        ] {
            let p: Preset = toml::from_str(&source.replace(from, to)).unwrap();
            assert!(p.validate().is_err());
        }
    }
}
