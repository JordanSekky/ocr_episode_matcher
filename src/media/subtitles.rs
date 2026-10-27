use anyhow::{Context, Result};
use clap::ValueEnum;
use image::{imageops, Rgb, RgbImage, RgbaImage};
use indicatif::{ProgressBar, ProgressStyle};
use pgs_rs::parse::{parse_pgs, PaletteEntry, Pgs, SegmentContents};
use pgs_rs::render::{render_display_set, DisplaySetIterator};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::media::{ffmpeg, ocr};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SubtitleCodec {
    /// Text subtitles (subrip), including the closed captions MakeMKV adds when ripping a DVD
    #[value(name = "subrip")]
    Srt,
    /// Blu-ray bitmap subtitles (hdmv_pgs_subtitle), read with OCR
    #[value(name = "pgs")]
    Pgs,
    /// DVD bitmap subtitles (dvd_subtitle), read with OCR
    #[value(name = "dvdsub")]
    Dvd,
}

impl SubtitleCodec {
    fn from_ffmpeg_name(name: &str) -> Option<Self> {
        match name {
            "subrip" => Some(Self::Srt),
            "hdmv_pgs_subtitle" => Some(Self::Pgs),
            "dvd_subtitle" => Some(Self::Dvd),
            _ => None,
        }
    }

    /// Lower is better: the preferred codec first, then text before the bitmap formats.
    fn rank(self, preferred: Self) -> u8 {
        if self == preferred {
            return 0;
        }
        match self {
            Self::Srt => 1,
            Self::Pgs => 2,
            Self::Dvd => 3,
        }
    }
}

pub struct SubtitleTrack {
    pub index: u32,
    pub codec: SubtitleCodec,
}

/// Picks the English subtitle track to read, favouring `preferred` and falling back to the
/// other supported codecs when the file has no such track.
pub fn find_best_subtitle_track(path: &Path, preferred: SubtitleCodec) -> Result<SubtitleTrack> {
    let json_output = ffmpeg::get_streams_json(path)?;
    let info: FfprobeOutput = serde_json::from_slice(&json_output)?;

    select_track(info, preferred)
        .context("No suitable English subtitle track found (subrip, PGS or DVD)")
}

fn select_track(info: FfprobeOutput, preferred: SubtitleCodec) -> Option<SubtitleTrack> {
    info.streams
        .into_iter()
        .filter(|stream| {
            stream
                .tags
                .as_ref()
                .and_then(|t| t.language.as_deref())
                .is_some_and(|l| l == "eng")
        })
        .filter_map(|stream| {
            let codec = SubtitleCodec::from_ffmpeg_name(&stream.codec_name)?;
            Some(SubtitleTrack {
                index: stream.index,
                codec,
            })
        })
        // min_by_key keeps the first of equals, so the earliest track of a codec wins
        .min_by_key(|track| track.codec.rank(preferred))
}

pub fn extract_subtitles(
    path: &Path,
    track_index: u32,
    codec: &SubtitleCodec,
    temp_dir: &Path,
    start_seconds: u64,
    duration_seconds: u64,
) -> Result<PathBuf> {
    let ext = match codec {
        SubtitleCodec::Srt => "srt",
        SubtitleCodec::Pgs => "sup",
        SubtitleCodec::Dvd => {
            // ffmpeg can't copy DVD subtitles into a standalone file, so render them to images
            let frames_dir = temp_dir.join("dvdsub_frames");
            fs::create_dir(&frames_dir)?;
            ffmpeg::extract_subtitle_frames(
                path,
                track_index,
                &frames_dir,
                start_seconds,
                duration_seconds,
            )?;
            return Ok(frames_dir);
        }
    };

    let output_path = temp_dir.join(format!("extracted.{ext}"));
    ffmpeg::extract_subtitle_track(
        path,
        track_index,
        &output_path,
        start_seconds,
        duration_seconds,
    )?;

    Ok(output_path)
}

/// Reads the extracted subtitles into plain text lines (no SRT indexes or timestamps).
/// Bitmap tracks (PGS, DVD) are OCR'd across all cores, one Tesseract engine per thread.
pub fn extract_text(subtitle_path: &Path, codec: &SubtitleCodec) -> Result<Vec<String>> {
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
            let mut pgs =
                parse_pgs(&mut data).map_err(|e| anyhow::anyhow!("Failed to parse PGS: {e:?}"))?;
            fill_transparent_palette_entries(&mut pgs);

            let display_sets: Vec<_> = DisplaySetIterator::new(&pgs)
                .filter(|ds| !ds.is_empty())
                .collect();
            let progress = ocr_progress_bar(display_sets.len(), "OCR of PGS subtitles");

            let texts = ocr_in_parallel(&display_sets, &progress, |ds| {
                let rgba_buffer = match render_display_set(ds) {
                    Ok(buffer) => buffer,
                    Err(e) => {
                        progress.suspend(|| {
                            eprintln!("Warning: Failed to render a PGS subtitle image: {e}")
                        });
                        return None;
                    }
                };

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
                Some((rgb_data, ds.width as i32, ds.height as i32))
            });
            progress.finish_and_clear();
            push_cues(&mut lines, texts?);
        }
        SubtitleCodec::Dvd => {
            // Frame names are zero-padded presentation times, so name order is playback order
            let mut frames: Vec<PathBuf> = fs::read_dir(subtitle_path)?
                .map(|entry| entry.map(|e| e.path()))
                .collect::<std::io::Result<_>>()?;
            frames.sort();

            let progress = ocr_progress_bar(frames.len(), "OCR of DVD subtitles");

            let texts = ocr_in_parallel(&frames, &progress, |frame| {
                let rgba = match image::open(frame) {
                    Ok(img) => img.to_rgba8(),
                    Err(e) => {
                        progress.suspend(|| {
                            eprintln!("Warning: Failed to read subtitle frame {frame:?}: {e}")
                        });
                        return None;
                    }
                };
                // Most frames are the blank ones ffmpeg emits when a subtitle ends
                let prepared = prepare_dvd_frame(&rgba)?;
                let (width, height) = prepared.dimensions();
                Some((prepared.into_raw(), width as i32, height as i32))
            });
            progress.finish_and_clear();
            push_cues(&mut lines, texts?);
        }
    }

    Ok(lines)
}

/// Adds every palette entry a PGS palette leaves out, as fully transparent.
///
/// Encoders may leave out the entries for the transparent background (0 and 255 on the Agents
/// of S.H.I.E.L.D. season 1 discs). Those entries are meant to be transparent, but pgs-rs fails
/// to render an image that uses an undefined entry.
fn fill_transparent_palette_entries(pgs: &mut Pgs) {
    for segment in &mut pgs.segments {
        if let SegmentContents::PaletteDefinition(palette) = &mut segment.contents {
            for id in 0..=u8::MAX {
                palette.entries.entry(id).or_insert(PaletteEntry {
                    id,
                    luminance: 0,
                    color_difference_red: 128,
                    color_difference_blue: 128,
                    alpha: 0,
                });
            }
        }
    }
}

/// The text found for the item at this index, if any.
type OcrResult = (usize, Option<String>);

/// Prepares each item into an RGB image (data, width, height) and OCRs it on all cores,
/// returning the text per item in the items' order. `None` where there was nothing to read.
///
/// The bundled Tesseract is built without OpenMP, so one engine only ever uses one core. Each
/// worker owns an engine and drops it before it exits, which Tesseract needs: engines still alive
/// at process exit make its shared cache print leak warnings.
fn ocr_in_parallel<T: Sync>(
    items: &[T],
    progress: &ProgressBar,
    prepare: impl Fn(&T) -> Option<(Vec<u8>, i32, i32)> + Sync,
) -> Result<Vec<Option<String>>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let workers = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    let next = AtomicUsize::new(0);

    let per_worker: Vec<Result<Vec<OcrResult>>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let api = ocr::create_ocr_engine()?;
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            break;
                        };
                        let text = prepare(item).and_then(|(rgb_data, width, height)| {
                            ocr_image(&api, &rgb_data, width, height)
                        });
                        progress.inc(1);
                        done.push((index, text));
                    }
                    Ok(done)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("OCR worker panicked"))
            .collect()
    });

    let mut texts = vec![None; items.len()];
    for worker in per_worker {
        for (index, text) in worker? {
            texts[index] = text;
        }
    }
    Ok(texts)
}

/// Appends each OCR'd cue followed by a blank line, like the cues of an SRT file.
fn push_cues(lines: &mut Vec<String>, texts: Vec<Option<String>>) {
    for text in texts.into_iter().flatten() {
        lines.push(text);
        lines.push(String::new());
    }
}

/// A progress bar for OCR'ing `total` subtitle images, drawn on stderr.
fn ocr_progress_bar(total: usize, message: &'static str) -> ProgressBar {
    let progress = ProgressBar::new(total as u64);
    progress.set_style(
        ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len} ({elapsed} elapsed, ETA {eta})")
            .expect("valid progress template")
            .progress_chars("=> "),
    );
    progress.set_message(message);
    progress
}

/// Space kept around the text so Tesseract doesn't see it touching the image edge.
const DVD_FRAME_MARGIN: u32 = 12;
/// DVD subtitles are only about 20px tall, which Tesseract reads poorly without upscaling.
const DVD_FRAME_SCALE: u32 = 3;

/// Turns a rendered DVD subtitle frame (light text with a dark outline on transparency) into
/// dark text on white, cropped to the text and enlarged. `None` if the frame is blank.
fn prepare_dvd_frame(frame: &RgbaImage) -> Option<RgbImage> {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in frame.enumerate_pixels() {
        if pixel[3] > 0 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if min_x > max_x {
        return None;
    }

    let width = max_x - min_x + 1 + 2 * DVD_FRAME_MARGIN;
    let height = max_y - min_y + 1 + 2 * DVD_FRAME_MARGIN;
    let mut canvas = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));

    // Composite over black, then invert: the outline vanishes into the white background
    for (x, y, pixel) in frame.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }
        let alpha = pixel[3] as u16;
        let out = canvas.get_pixel_mut(x - min_x + DVD_FRAME_MARGIN, y - min_y + DVD_FRAME_MARGIN);
        for channel in 0..3 {
            out[channel] = 255 - ((pixel[channel] as u16 * alpha) / 255) as u8;
        }
    }

    Some(imageops::resize(
        &canvas,
        width * DVD_FRAME_SCALE,
        height * DVD_FRAME_SCALE,
        imageops::FilterType::Lanczos3,
    ))
}

/// Runs Tesseract over an RGB image, returning the cleaned text if there is any.
fn ocr_image(
    api: &tesseract_rs::TesseractAPI,
    rgb_data: &[u8],
    width: i32,
    height: i32,
) -> Option<String> {
    api.set_image(rgb_data, width, height, 3, 3 * width).ok()?;
    let text = api.get_utf8_text().ok()?;
    let cleaned = clean_ocr_text(&text);
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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

    #[test]
    fn test_fill_transparent_palette_entries() {
        use pgs_rs::parse::{PaletteDefinition, Segment};
        use std::collections::HashMap;

        let defined = PaletteEntry {
            id: 1,
            luminance: 200,
            color_difference_red: 120,
            color_difference_blue: 130,
            alpha: 255,
        };
        let mut pgs = Pgs {
            segments: vec![Segment {
                pts: 0,
                dts: 0,
                contents: SegmentContents::PaletteDefinition(PaletteDefinition {
                    id: 0,
                    version: 0,
                    entries: HashMap::from([(1, defined)]),
                }),
            }],
        };

        fill_transparent_palette_entries(&mut pgs);

        let SegmentContents::PaletteDefinition(palette) = &pgs.segments[0].contents else {
            panic!("expected a palette");
        };
        assert_eq!(palette.entries.len(), 256);
        // What the encoder defined is left alone; what it left out is invisible.
        assert_eq!(palette.entries[&1].luminance, 200);
        assert_eq!(palette.entries[&1].alpha, 255);
        assert_eq!(palette.entries[&0].alpha, 0);
        assert_eq!(palette.entries[&255].alpha, 0);
    }

    fn streams(json: &str) -> FfprobeOutput {
        serde_json::from_str(json).unwrap()
    }

    /// The Scooby-Doo rip: three DVD bitmap tracks, then MakeMKV's closed-caption subrip track
    const DVD_RIP: &str = r#"{"streams": [
        {"index": 2, "codec_name": "dvd_subtitle", "tags": {"language": "eng"}},
        {"index": 3, "codec_name": "dvd_subtitle", "tags": {"language": "fre"}},
        {"index": 4, "codec_name": "dvd_subtitle", "tags": {"language": "spa"}},
        {"index": 5, "codec_name": "subrip", "tags": {"language": "eng"}}
    ]}"#;

    #[test]
    fn test_select_track_prefers_requested_codec() {
        let track = select_track(streams(DVD_RIP), SubtitleCodec::Dvd).unwrap();
        assert_eq!((track.index, track.codec), (2, SubtitleCodec::Dvd));

        let track = select_track(streams(DVD_RIP), SubtitleCodec::Srt).unwrap();
        assert_eq!((track.index, track.codec), (5, SubtitleCodec::Srt));
    }

    #[test]
    fn test_select_track_falls_back_when_preferred_is_missing() {
        // No PGS track: subrip beats DVD
        let track = select_track(streams(DVD_RIP), SubtitleCodec::Pgs).unwrap();
        assert_eq!(track.codec, SubtitleCodec::Srt);

        let only_dvd = r#"{"streams": [
            {"index": 1, "codec_name": "dvd_subtitle", "tags": {"language": "eng"}}
        ]}"#;
        let track = select_track(streams(only_dvd), SubtitleCodec::Srt).unwrap();
        assert_eq!((track.index, track.codec), (1, SubtitleCodec::Dvd));
    }

    #[test]
    fn test_select_track_ignores_other_languages_and_codecs() {
        let json = r#"{"streams": [
            {"index": 1, "codec_name": "dvd_subtitle", "tags": {"language": "fre"}},
            {"index": 2, "codec_name": "ass", "tags": {"language": "eng"}},
            {"index": 3, "codec_name": "subrip"}
        ]}"#;
        assert!(select_track(streams(json), SubtitleCodec::Srt).is_none());
    }

    #[test]
    fn test_select_track_keeps_first_of_equal_tracks() {
        let json = r#"{"streams": [
            {"index": 4, "codec_name": "subrip", "tags": {"language": "eng"}},
            {"index": 6, "codec_name": "subrip", "tags": {"language": "eng"}}
        ]}"#;
        assert_eq!(
            select_track(streams(json), SubtitleCodec::Srt)
                .unwrap()
                .index,
            4
        );
    }

    #[test]
    fn test_prepare_dvd_frame() {
        use image::Rgba;

        let blank = RgbaImage::new(720, 480);
        assert!(prepare_dvd_frame(&blank).is_none());

        // White glyph pixel with a black outline pixel beside it
        let mut frame = RgbaImage::new(720, 480);
        frame.put_pixel(100, 400, Rgba([255, 255, 255, 255]));
        frame.put_pixel(101, 400, Rgba([0, 0, 0, 255]));
        let prepared = prepare_dvd_frame(&frame).unwrap();

        // Cropped to the two pixels plus margins, then enlarged
        assert_eq!(
            prepared.dimensions(),
            (
                (2 + 2 * DVD_FRAME_MARGIN) * DVD_FRAME_SCALE,
                (1 + 2 * DVD_FRAME_MARGIN) * DVD_FRAME_SCALE
            )
        );
        // Dark text on a white background
        assert_eq!(prepared.get_pixel(0, 0), &Rgb([255, 255, 255]));
        let centre = prepared.get_pixel(
            DVD_FRAME_MARGIN * DVD_FRAME_SCALE + 1,
            (DVD_FRAME_MARGIN + 1) * DVD_FRAME_SCALE - 2,
        );
        assert!(centre[0] < 128, "glyph should be dark, got {centre:?}");
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
