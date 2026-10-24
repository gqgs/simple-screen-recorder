//! End-to-end checks with synthetic media, no desktop access required.
use std::{
    path::Path,
    process::{Command, Output},
};

fn run(command: &mut Command) -> Output {
    let result = command.output().expect("command available");
    assert!(
        result.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
fn probe(path: &Path) -> String {
    String::from_utf8(
        run(Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,width,height,pix_fmt,r_frame_rate",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(path))
        .stdout,
    )
    .unwrap()
}
#[test]
#[ignore = "requires GStreamer capture plugins, FFmpeg and ffprobe"]
fn rectangle_capture_exports_and_protects_output() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("rectangle with spaces.mp4");
    run(Command::new(env!("CARGO_BIN_EXE_ssr"))
        .args([
            "record",
            "--test-source",
            "--duration",
            "1",
            "--region",
            "11,21,321,181",
            "-o",
        ])
        .arg(&output));
    let info = probe(&output);
    assert!(info.contains("codec_name=h264"), "{info}");
    assert!(info.contains("width=320\nheight=180"), "{info}");
    assert!(info.contains("pix_fmt=yuv420p"), "{info}");
    assert!(info.contains("r_frame_rate=60/1"), "{info}");
    let before = std::fs::read(&output).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_ssr"))
        .args(["record", "--test-source", "--duration", "1", "-o"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert_eq!(before, std::fs::read(&output).unwrap());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn export_preserves_audio_and_applies_custom_preset() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.mkv");
    let output = dir.path().join("output.mp4");
    let preset = dir.path().join("custom.toml");
    std::fs::write(
        &preset,
        include_str!("../presets/twitter-30.toml")
            .replace("1920", "320")
            .replace("1080", "180"),
    )
    .unwrap();
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x480:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "1",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&input));
    run(Command::new(env!("CARGO_BIN_EXE_ssr"))
        .arg("export")
        .arg(input)
        .arg("--preset")
        .arg(preset)
        .arg("-o")
        .arg(&output));
    let info = probe(&output);
    assert!(info.contains("codec_name=aac"), "{info}");
    assert!(info.contains("width=240\nheight=180"), "{info}");
    assert!(info.contains("r_frame_rate=30/1"), "{info}");
}

#[test]
#[ignore = "requires GStreamer capture plugins, FFmpeg, ffprobe and POSIX kill"]
fn ctrl_c_finalizes_a_playable_recording() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("stopped.mp4");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ssr"))
        .args(["record", "--test-source", "--duration", "10", "-o"])
        .arg(&output)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            stderr.read_line(&mut line).unwrap() > 0,
            "recorder exited before starting"
        );
        if line.starts_with("Recording.") {
            break;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    run(Command::new("kill").args(["-INT", &child.id().to_string()]));
    let mut remaining = String::new();
    std::io::Read::read_to_string(&mut stderr, &mut remaining).unwrap();
    assert!(child.wait().unwrap().success(), "{remaining}");
    assert!(
        remaining.contains("Finalizing capture"),
        "SIGINT was not handled: {remaining}"
    );
    assert!(probe(&output).contains("codec_name=h264"));
}
