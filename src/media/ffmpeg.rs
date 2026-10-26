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

/// Length of the file in whole minutes, rounded to the nearest.
pub fn get_duration_minutes(path: &Path) -> Result<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
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

    parse_duration_minutes(&String::from_utf8_lossy(&output.stdout))
}

fn parse_duration_minutes(ffprobe_output: &str) -> Result<u64> {
    let seconds: f64 = ffprobe_output
        .trim()
        .parse()
        .with_context(|| format!("Unexpected ffprobe duration: {:?}", ffprobe_output.trim()))?;
    if !seconds.is_finite() || seconds < 0.0 {
        bail!("Unexpected ffprobe duration: {seconds}");
    }
    Ok((seconds / 60.0).round() as u64)
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

    let output = Command::new("ffmpeg")
        .args([
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
    let output = Command::new("ffmpeg")
        .args([
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
        ])
        .output()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration_minutes("2640.512000\n").unwrap(), 44);
        assert_eq!(parse_duration_minutes("2669.9").unwrap(), 44);
        assert_eq!(parse_duration_minutes("2670").unwrap(), 45);
        assert_eq!(parse_duration_minutes("29.9").unwrap(), 0);
        assert_eq!(parse_duration_minutes("9000").unwrap(), 150);
    }

    #[test]
    fn test_parse_duration_minutes_invalid() {
        assert!(parse_duration_minutes("").is_err());
        assert!(parse_duration_minutes("N/A").is_err());
        assert!(parse_duration_minutes("-5").is_err());
        assert!(parse_duration_minutes("inf").is_err());
    }
}
