use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

/// Runs `command` to completion while showing a spinner labelled `message` on stderr.
fn run_with_spinner(command: &mut Command, message: &'static str) -> std::io::Result<Output> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("{spinner} {msg} ({elapsed})")
            .expect("valid spinner template"),
    );
    spinner.set_message(message);
    spinner.enable_steady_tick(Duration::from_millis(100));

    let output = command.output();
    spinner.finish_and_clear();
    output
}

pub fn get_streams_json(path: &Path) -> Result<Vec<u8>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "s",
            path.to_str().context("Invalid path")?,
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(output.stdout)
}

pub fn extract_subtitle_track(
    input_path: &Path,
    track_index: u32,
    output_path: &Path,
    start_seconds: u64,
    duration_seconds: u64,
) -> Result<()> {
    let output_str = output_path
        .to_str()
        .context("Invalid output path for subtitles")?;
    let start = start_seconds.to_string();
    let duration = duration_seconds.to_string();

    let output = run_with_spinner(
        Command::new("ffmpeg").args([
            "-y",
            "-ss",
            &start,
            "-i",
            input_path.to_str().context("Invalid input path")?,
            "-map",
            &format!("0:{track_index}"),
            "-c:s",
            "copy",
            "-t",
            &duration,
            output_str,
        ]),
        "Extracting subtitles with ffmpeg",
    )
    .context("Failed to run ffmpeg for subtitle extraction")?;

    if !output.status.success() {
        bail!(
            "ffmpeg subtitle extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Renders a bitmap subtitle track (e.g. `dvd_subtitle`) to RGBA PNGs in `output_dir`.
///
/// ffmpeg turns each subtitle event into a frame at its start and a blank frame at its end.
/// The files are named by presentation time so they sort in playback order.
pub fn extract_subtitle_frames(
    input_path: &Path,
    track_index: u32,
    output_dir: &Path,
    start_seconds: u64,
    duration_seconds: u64,
) -> Result<()> {
    let output_pattern = output_dir.join("%012d.png");
    let start = start_seconds.to_string();
    let duration = duration_seconds.to_string();

    // `-t` goes before `-i` because the rendered frames start at the first subtitle, not at 0,
    // so as an output option it would read `duration` seconds past the first subtitle.
    let output = run_with_spinner(
        Command::new("ffmpeg").args([
            "-y",
            "-ss",
            &start,
            "-t",
            &duration,
            "-i",
            input_path.to_str().context("Invalid input path")?,
            "-filter_complex",
            &format!("[0:{track_index}]copy[subs]"),
            "-map",
            "[subs]",
            "-an",
            "-fps_mode",
            "passthrough",
            "-frame_pts",
            "1",
            output_pattern
                .to_str()
                .context("Invalid output path for subtitle frames")?,
        ]),
        "Rendering subtitle frames with ffmpeg",
    )
    .context("Failed to run ffmpeg for subtitle frame extraction")?;

    if !output.status.success() {
        bail!(
            "ffmpeg subtitle frame extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

pub fn extract_frames(input_path: &str, output_pattern: &str) -> Result<()> {
    let ffmpeg_output = Command::new("ffmpeg")
        .arg("-sseof")
        .arg("-15")
        .arg("-i")
        .arg(input_path)
        .arg("-vf")
        .arg("fps=1")
        .arg("-y")
        .arg(output_pattern)
        .output();

    let ffmpeg_output = match ffmpeg_output {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("FFmpeg not found. Please install ffmpeg and ensure it's in your PATH.");
        }
        Err(e) => {
            bail!("Failed to execute ffmpeg: {e}");
        }
    };

    if !ffmpeg_output.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
        bail!("FFmpeg error: {stderr}");
    }

    Ok(())
}
