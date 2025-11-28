use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

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
) -> Result<()> {
    let output_str = output_path
        .to_str()
        .context("Invalid output path for subtitles")?;

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            input_path.to_str().context("Invalid input path")?,
            "-map",
            &format!("0:{track_index}"),
            "-c:s",
            "copy",
            output_str,
        ])
        .output()
        .context("Failed to run ffmpeg for subtitle extraction")?;

    if !output.status.success() {
        bail!(
            "ffmpeg subtitle extraction failed: {}",
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

