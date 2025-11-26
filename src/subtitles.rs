use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    index: usize,
    codec_name: String,
    codec_type: String,
    tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    pub index: usize,
    pub codec_name: String,
    pub language: Option<String>,
    pub is_text: bool,
}

pub fn get_subtitle_tracks(path: &str) -> Result<Vec<SubtitleTrack>> {
    let output = Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "s",
            path,
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let probe: FfprobeOutput =
        serde_json::from_slice(&output.stdout).context("Failed to parse ffprobe output")?;

    let tracks = probe
        .streams
        .into_iter()
        .filter(|s| s.codec_type == "subtitle")
        .map(|s| {
            let is_text = matches!(
                s.codec_name.as_str(),
                "subrip" | "ass" | "webvtt" | "mov_text" | "text"
            );

            let language = s
                .tags
                .as_ref()
                .and_then(|t| t.get("language"))
                .map(|l| l.to_lowercase());

            SubtitleTrack {
                index: s.index,
                codec_name: s.codec_name,
                language,
                is_text,
            }
        })
        .collect();

    Ok(tracks)
}

pub fn select_best_track(tracks: &[SubtitleTrack]) -> Option<&SubtitleTrack> {
    if tracks.is_empty() {
        return None;
    }

    // 1. Filter for English tracks if any exist
    let english_tracks: Vec<&SubtitleTrack> = tracks
        .iter()
        .filter(|t| t.language.as_deref() == Some("eng") || t.language.as_deref() == Some("en"))
        .collect();

    let pool = if !english_tracks.is_empty() {
        english_tracks
    } else {
        tracks.iter().collect()
    };

    // 2. Prefer text tracks over image tracks
    if let Some(text_track) = pool.iter().find(|t| t.is_text) {
        return Some(text_track);
    }

    // 3. Fallback to first available track
    pool.first().copied()
}

pub fn extract_and_display_subtitles(path: &Path, track: &SubtitleTrack) -> Result<()> {
    let pager = env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    if track.is_text {
        // For text tracks, pipe ffmpeg stdout -> pager stdin
        let mut pager_cmd = Command::new(&pager)
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to spawn pager")?;

        let pager_stdin = pager_cmd
            .stdin
            .take()
            .context("Failed to open pager stdin")?;

        let mut ffmpeg = Command::new("ffmpeg")
            .args(&[
                "-i",
                path.to_str().unwrap(),
                "-map",
                &format!("0:{}", track.index),
                "-f",
                "srt",
                "-",
            ])
            .stdout(pager_stdin) // Pipe directly to pager
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn ffmpeg")?;

        ffmpeg.wait()?;
        pager_cmd.wait()?;
    } else {
        extract_and_ocr_pgs(path, track, &pager)?;
    }

    Ok(())
}

fn extract_and_ocr_pgs(path: &Path, track: &SubtitleTrack, pager: &str) -> Result<()> {
    let temp_dir = TempDir::new()?;
    // Avoid keeping references to temp_dir.path() around
    let temp_path_buf = temp_dir.path().to_path_buf();
    let output_pattern_str = temp_path_buf
        .join("sub_%04d.png")
        .to_string_lossy()
        .to_string();

    // Spawn pager process
    let mut pager_cmd = Command::new(pager)
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn pager")?;

    let mut pager_stdin = pager_cmd
        .stdin
        .take()
        .context("Failed to open pager stdin")?;

    // Signal for ffmpeg completion
    let ffmpeg_finished = Arc::new(AtomicBool::new(false));
    let ffmpeg_finished_clone = ffmpeg_finished.clone();

    // Spawn ffmpeg to extract frames
    let path_str = path.to_str().unwrap().to_string();
    let track_index = track.index;

    thread::spawn(move || {
        let _ = Command::new("ffmpeg")
            .args(&[
                "-i",
                &path_str,
                "-map",
                &format!("0:{}", track_index),
                "-vsync",
                "0", // prevent duplicating frames
                "-y",
                &output_pattern_str,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        ffmpeg_finished_clone.store(true, Ordering::Release);
    });

    // OCR Thread: watch for files and write to pager
    let ocr_path_buf = temp_path_buf.clone();

    thread::spawn(move || -> Result<()> {
        let mut processed_count = 0;
        let api = crate::ocr::create_ocr_engine()?;
        let mut writer = BufWriter::new(&mut pager_stdin);

        // Give ffmpeg a moment to start
        thread::sleep(Duration::from_millis(500));

        loop {
            // Check for next file
            let next_file = ocr_path_buf.join(format!("sub_{:04}.png", processed_count + 1));

            if next_file.exists() {
                // Small delay to ensure file write is complete
                thread::sleep(Duration::from_millis(50));

                if let Ok(img) = image::open(&next_file) {
                    let rgb_img = img.to_rgb8();
                    let (width, height) = rgb_img.dimensions();
                    let image_data = rgb_img.into_raw();

                    if api
                        .set_image(
                            &image_data,
                            width as i32,
                            height as i32,
                            3,
                            3 * width as i32,
                        )
                        .is_ok()
                    {
                        if let Ok(text) = api.get_utf8_text() {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                // Write to pager, ignore broken pipe (pager closed)
                                if writeln!(writer, "{}\n", trimmed).is_err() {
                                    return Ok(());
                                }
                                if writer.flush().is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }

                // Clean up
                let _ = fs::remove_file(next_file);
                processed_count += 1;
            } else {
                // Check if ffmpeg is done
                if ffmpeg_finished.load(Ordering::Acquire) {
                    // Double check one last time for straggling files
                    thread::sleep(Duration::from_millis(200));
                    if !ocr_path_buf
                        .join(format!("sub_{:04}.png", processed_count + 1))
                        .exists()
                    {
                        break;
                    }
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Ok(())
    });

    // Wait for pager to exit (user quits)
    pager_cmd.wait()?;

    Ok(())
}
