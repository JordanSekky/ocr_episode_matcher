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
    duration_seconds: u64,
) -> Result<std::path::PathBuf> {
    let ext = match codec {
        SubtitleCodec::Srt => "srt",
        SubtitleCodec::Pgs => "sup",
    };

    let output_path = temp_dir.join(format!("extracted.{ext}"));
    ffmpeg::extract_subtitle_track(path, track_index, &output_path, duration_seconds)?;

    Ok(output_path)
}

/// Reads the extracted subtitle file into plain text lines (no SRT indexes or timestamps).
/// PGS tracks are OCR'd, so `ocr_engine` is required for them.
pub fn extract_text(
    subtitle_path: &Path,
    codec: &SubtitleCodec,
    ocr_engine: Option<tesseract_rs::TesseractAPI>,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();

    match codec {
        SubtitleCodec::Srt => {
            let file = File::open(subtitle_path)?;
            for line in BufReader::new(file).lines() {
                push_srt_line(&mut lines, line?);
            }
        }
        SubtitleCodec::Pgs => {
            let mut data = fs::read(subtitle_path)?;
            let pgs =
                parse_pgs(&mut data).map_err(|e| anyhow::anyhow!("Failed to parse PGS: {e:?}"))?;

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
                            let cleaned_text = clean_ocr_text(&text);
                            let trimmed = cleaned_text.trim();
                            if !trimmed.is_empty() {
                                lines.push(trimmed.to_string());
                                lines.push(String::new());
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(lines)
}

/// Appends an SRT line, dropping cue timestamps and the numeric cue index before them.
fn push_srt_line(lines: &mut Vec<String>, line: String) {
    let line = line.trim_start_matches('\u{feff}').trim_end().to_string();

    if line.contains("-->") {
        if lines
            .last()
            .is_some_and(|prev| !prev.is_empty() && prev.chars().all(|c| c.is_ascii_digit()))
        {
            lines.pop();
        }
        return;
    }

    // Collapse runs of blank lines between cues
    if line.is_empty() && lines.last().is_none_or(|prev| prev.is_empty()) {
        return;
    }
    lines.push(line);
}

pub fn display_lines(lines: &[String]) -> Result<()> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    let mut child = Command::new(pager)
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn pager")?;

    let mut stdin = child.stdin.take().context("Failed to open pager stdin")?;

    for line in lines {
        if writeln!(stdin, "{line}").is_err() {
            break; // Pager closed
        }
    }

    // Drop stdin to close the pipe and signal EOF to the pager
    drop(stdin);
    let _ = child.wait();
    Ok(())
}

/// Replaces the `|` that Tesseract emits for a capital I and strips anything unprintable.
fn clean_ocr_text(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '|' => 'I', // Replace pipe with capital I
            _ => c,
        })
        .filter(|c| {
            c.is_alphanumeric()
                || c.is_whitespace()
                || "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".contains(*c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subtitle_text_cleaning() {
        let input = "Hello | World! @#$%^&*()";
        // | -> I
        // @#$%^&*() are allowed punctuation
        assert_eq!(clean_ocr_text(input), "Hello I World! @#$%^&*()");

        let input_with_invalid = "Hello \u{0000} World \u{001F}"; // Null and Unit Separator
        let expected_cleaned = "Hello  World ";

        // Note: The whitespace check allows spaces, so the control chars are removed but spaces remain
        assert_eq!(
            clean_ocr_text(input_with_invalid).trim(),
            expected_cleaned.trim()
        );
    }

    fn srt_lines(input: &str) -> Vec<String> {
        let mut lines = Vec::new();
        for line in input.lines() {
            push_srt_line(&mut lines, line.to_string());
        }
        lines
    }

    #[test]
    fn test_srt_strips_indexes_and_timestamps() {
        let input = "1\n00:00:01,000 --> 00:00:03,000\nHello there.\n\n2\n00:00:04,000 --> 00:00:06,000\n42\nGeneral Kenobi.\n";
        assert_eq!(
            srt_lines(input),
            vec!["Hello there.", "", "42", "General Kenobi."]
        );
    }

    #[test]
    fn test_srt_collapses_blank_lines_and_strips_bom() {
        let input = "\u{feff}1\n00:00:01,000 --> 00:00:03,000\nOne\n\n\n\n2\n00:00:04,000 --> 00:00:06,000\nTwo\n";
        assert_eq!(srt_lines(input), vec!["One", "", "Two"]);
    }
}
