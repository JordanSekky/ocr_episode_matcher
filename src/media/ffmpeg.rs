use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::borrow::Cow;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

/// Runs `command` to completion while showing a spinner labelled `message` on stderr.
fn run_with_spinner(
    command: &mut Command,
    message: impl Into<Cow<'static, str>>,
) -> std::io::Result<Output> {
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

/// A chapter's span within a file, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

pub fn get_chapters(path: &Path) -> Result<Vec<Chapter>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_chapters",
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

    parse_chapters(&output.stdout)
}

fn parse_chapters(json: &[u8]) -> Result<Vec<Chapter>> {
    // ffprobe prints the times as strings of seconds, e.g. "464.864400"
    #[derive(Deserialize)]
    struct RawChapter {
        start_time: String,
        end_time: String,
    }
    #[derive(Deserialize)]
    struct Chapters {
        chapters: Vec<RawChapter>,
    }

    let parsed: Chapters =
        serde_json::from_slice(json).context("Failed to parse ffprobe chapter output")?;
    parsed
        .chapters
        .into_iter()
        .map(|c| {
            Ok(Chapter {
                start_seconds: c.start_time.parse().context("Invalid chapter start time")?,
                end_seconds: c.end_time.parse().context("Invalid chapter end time")?,
            })
        })
        .collect()
}

/// Larger than the millisecond rounding of timestamps, far smaller than a frame.
const CHAPTER_MARGIN_SECONDS: f64 = 0.01;

/// Copies each of `chapters` of `input_path` into its own file, without re-encoding and keeping
/// every stream. `output_pattern` holds a printf-style number (e.g. `disc_ch%02d.mkv`) that
/// ffmpeg fills in from 1. Fails rather than overwrite an existing file.
///
/// The chapters must be contiguous and cover the file, as the chapters of a disc title do: only
/// where each one starts is used.
pub fn split_at_chapters(
    input_path: &Path,
    chapters: &[Chapter],
    output_pattern: &Path,
    message: String,
) -> Result<()> {
    // The segment muxer starts a new file at the first keyframe at or after each time. A chapter
    // starts on a keyframe, but ffprobe reports it to the nanosecond while the keyframe's
    // timestamp is rounded to the millisecond, so the keyframe can fall a hair before the chapter
    // start and be missed in favour of the next one. Asking for a little earlier is still
    // nearer to this keyframe than to the one before it.
    let times = chapters
        .iter()
        .skip(1)
        .map(|c| format!("{:.6}", c.start_seconds - CHAPTER_MARGIN_SECONDS))
        .collect::<Vec<_>>()
        .join(",");

    // Segmenting at keyframes by presentation time is what keeps the first frames of the next
    // chapter out of the end of this one when the video has B-frames. `-map_chapters -1` stops
    // each piece from carrying all of the source's chapter marks.
    let output = run_with_spinner(
        Command::new("ffmpeg").args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-n",
            "-i",
            input_path.to_str().context("Invalid input path")?,
            "-map",
            "0",
            "-c",
            "copy",
            "-map_chapters",
            "-1",
            "-f",
            "segment",
            "-segment_times",
            &times,
            "-segment_start_number",
            "1",
            "-reset_timestamps",
            "1",
            output_pattern.to_str().context("Invalid output path")?,
        ]),
        message,
    )
    .context("Failed to run ffmpeg for chapter extraction")?;

    if !output.status.success() {
        bail!(
            "ffmpeg chapter extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
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

/// Writes a frame a second from `duration_seconds` of `input_path`, starting `start_seconds` in,
/// as PNGs named by `output_pattern` (e.g. `frame_%04d.png`).
///
/// DVD video has non-square pixels, so the frames are scaled to the shape they are shown in,
/// which also gives OCR the proportions the lettering was drawn with.
pub fn extract_frames_between(
    input_path: &Path,
    start_seconds: u64,
    duration_seconds: u64,
    output_pattern: &Path,
) -> Result<()> {
    let start = start_seconds.to_string();
    let duration = duration_seconds.to_string();

    let output = run_with_spinner(
        Command::new("ffmpeg").args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            &start,
            "-i",
            input_path.to_str().context("Invalid input path")?,
            "-t",
            &duration,
            "-an",
            "-sn",
            "-vf",
            "fps=1,scale=iw*sar:ih,setsar=1",
            output_pattern.to_str().context("Invalid output path")?,
        ]),
        "Extracting frames with ffmpeg",
    )
    .context("Failed to run ffmpeg for frame extraction")?;

    if !output.status.success() {
        bail!(
            "ffmpeg frame extraction failed: {}",
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
    fn parse_chapters_reads_start_and_end_in_seconds() {
        let json = br#"{"chapters": [
            {"id": 1, "start": 0, "start_time": "0.000000", "end": 464864400000, "end_time": "464.864400"},
            {"id": 2, "start": 464864400000, "start_time": "464.864400", "end": 910509600000, "end_time": "910.509600"}
        ]}"#;
        assert_eq!(
            parse_chapters(json).unwrap(),
            vec![
                Chapter {
                    start_seconds: 0.0,
                    end_seconds: 464.8644
                },
                Chapter {
                    start_seconds: 464.8644,
                    end_seconds: 910.5096
                },
            ]
        );
    }

    #[test]
    fn parse_chapters_of_a_file_without_chapters_is_empty() {
        assert!(parse_chapters(br#"{"chapters": []}"#).unwrap().is_empty());
    }
}
