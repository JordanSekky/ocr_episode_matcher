use anyhow::{bail, Context, Result};
use pgs_rs::parse::parse_pgs;
use pgs_rs::render::{render_display_set, DisplaySetIterator};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    index: u32,
    codec_name: String,
    tags: Option<Tags>,
}

#[derive(Debug, Deserialize)]
struct Tags {
    language: Option<String>,
}

#[derive(Debug)]
pub enum SubtitleCodec {
    Srt, // subrip
    Pgs, // hdmv_pgs_subtitle
}

pub struct SubtitleTrack {
    pub index: u32,
    pub codec: SubtitleCodec,
}

pub fn find_best_subtitle_track(path: &Path) -> Result<SubtitleTrack> {
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
        bail!("ffprobe failed");
    }

    let info: FfprobeOutput = serde_json::from_slice(&output.stdout)?;

    let mut best_track: Option<SubtitleTrack> = None;

    for stream in info.streams {
        // Check for English language
        let is_eng = stream
            .tags
            .as_ref()
            .and_then(|t| t.language.as_ref())
            .map(|l| l == "eng")
            .unwrap_or(false);

        if !is_eng {
            continue;
        }

        let codec = match stream.codec_name.as_str() {
            "subrip" => SubtitleCodec::Srt,
            "hdmv_pgs_subtitle" => SubtitleCodec::Pgs,
            _ => continue,
        };

        // Prioritize SRT over PGS
        match (codec, &best_track) {
            (SubtitleCodec::Srt, _) => {
                return Ok(SubtitleTrack {
                    index: stream.index,
                    codec: SubtitleCodec::Srt,
                });
            }
            (SubtitleCodec::Pgs, None) => {
                best_track = Some(SubtitleTrack {
                    index: stream.index,
                    codec: SubtitleCodec::Pgs,
                });
            }
            _ => {}
        }
    }

    best_track.context("No suitable English subtitle track found (SRT or PGS)")
}

pub fn extract_subtitles(
    path: &Path,
    track_index: u32,
    codec: &SubtitleCodec,
    temp_dir: &Path,
) -> Result<std::path::PathBuf> {
    let ext = match codec {
        SubtitleCodec::Srt => "srt",
        SubtitleCodec::Pgs => "sup",
    };

    let output_path = temp_dir.join(format!("extracted.{}", ext));
    let output_str = output_path
        .to_str()
        .context("Invalid output path for subtitles")?;

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            path.to_str().context("Invalid input path")?,
            "-map",
            &format!("0:{}", track_index),
            "-c:s",
            "copy",
            output_str,
        ])
        .status()
        .context("Failed to run ffmpeg for subtitle extraction")?;

    if !status.success() {
        bail!("ffmpeg subtitle extraction failed");
    }

    Ok(output_path)
}

pub fn process_and_display(
    subtitle_path: &Path,
    codec: &SubtitleCodec,
    ocr_engine: Option<tesseract_rs::TesseractAPI>,
) -> Result<()> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    let mut child = Command::new(pager)
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn pager")?;

    let mut stdin = child.stdin.take().context("Failed to open pager stdin")?;

    match codec {
        SubtitleCodec::Srt => {
            let file = File::open(subtitle_path)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if writeln!(stdin, "{}", line).is_err() {
                    break; // Pager closed
                }
            }
        }
        SubtitleCodec::Pgs => {
            let mut data = fs::read(subtitle_path)?;
            let pgs = parse_pgs(&mut data)
                .map_err(|e| anyhow::anyhow!("Failed to parse PGS: {:?}", e))?;

            // We need the OCR engine for PGS
            let api = ocr_engine.context("OCR engine required for PGS subtitles")?;

            for ds in DisplaySetIterator::new(&pgs) {
                if ds.is_empty() {
                    continue;
                }

                let _ = writeln!(
                    stdin,
                    "{}/{}:{}\n",
                    ds.composition_number,
                    pgs.segments.len(),
                    ds.presentation_timestamp
                );

                if let Ok(rgba_buffer) = render_display_set(&ds) {
                    let width = ds.width as i32;
                    let height = ds.height as i32;

                    if api
                        .set_image(&rgba_buffer, width, height, 4, 4 * width)
                        .is_ok()
                    {
                        if let Ok(text) = api.get_utf8_text() {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                if writeln!(stdin, "{}", trimmed).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // Drop stdin to close the pipe and signal EOF to the pager
            drop(stdin);
        }
    }

    let _ = child.wait();
    Ok(())
}
