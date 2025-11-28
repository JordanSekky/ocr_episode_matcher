use anyhow::{Context, Result};
use pgs_rs::parse::parse_pgs;
use pgs_rs::render::{render_display_set, DisplaySetIterator};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::media::ffmpeg;

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
    let json_output = ffmpeg::get_streams_json(path)?;
    let info: FfprobeOutput = serde_json::from_slice(&json_output)?;

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

    let output_path = temp_dir.join(format!("extracted.{ext}"));
    ffmpeg::extract_subtitle_track(path, track_index, &output_path)?;

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
                if writeln!(stdin, "{line}").is_err() {
                    break; // Pager closed
                }
            }
        }
        SubtitleCodec::Pgs => {
            let mut data = fs::read(subtitle_path)?;
            let pgs = parse_pgs(&mut data)
                .map_err(|e| anyhow::anyhow!("Failed to parse PGS: {e:?}"))?;

            // We need the OCR engine for PGS
            let api = ocr_engine.context("OCR engine required for PGS subtitles")?;

            for ds in DisplaySetIterator::new(&pgs) {
                if ds.is_empty() {
                    continue;
                }

                if let Ok(rgba_buffer) = render_display_set(&ds) {
                    let width = ds.width as i32;
                    let height = ds.height as i32;

                    // Convert RGBA to RGB, compositing over a black background
                    let rgb_data: Vec<u8> = rgba_buffer
                        .chunks(4)
                        .flat_map(|chunk| {
                            // chunk is [r, g, b, a]
                            // Alpha composition: output = color * alpha + background * (1 - alpha)
                            // Since background is black (0), output = color * alpha
                            let r = chunk[0] as u16;
                            let g = chunk[1] as u16;
                            let b = chunk[2] as u16;
                            let a = chunk[3] as u16;

                            [
                                ((r * a) / 255) as u8,
                                ((g * a) / 255) as u8,
                                ((b * a) / 255) as u8,
                            ]
                        })
                        .collect();

                    if api
                        .set_image(&rgb_data, width, height, 3, 3 * width)
                        .is_ok()
                    {
                        if let Ok(text) = api.get_utf8_text() {
                            let trimmed = text.trim();
                            if !trimmed.is_empty()
                                && writeln!(stdin, "{trimmed}\n").is_err() {
                                    break;
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
