use anyhow::{bail, Result};
use image::{imageops::FilterType, DynamicImage};
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use tempfile::TempDir;
use tesseract_rs::{TessPageSegMode, TesseractAPI};

use crate::media::ffmpeg;

#[cfg(target_os = "macos")]
const ENG: &[u8] = include_bytes!(concat!(
    env!("HOME"),
    "/Library/Application Support/tesseract-rs/tessdata/eng.traineddata"
));

#[cfg(target_os = "linux")]
const ENG: &[u8] = include_bytes!(concat!(
    env!("HOME"),
    "/.tesseract-rs/tessdata/eng.traineddata"
));

#[cfg(target_os = "windows")]
const ENG: &[u8] = include_bytes!(concat!(
    env!("APPDATA"),
    "/tesseract-rs/tessdata/eng.traineddata"
));

pub fn extract_production_code_candidates(mkv_path: &str) -> Result<Vec<String>> {
    // Create temporary directory for frames
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Extract frames from last 15 seconds at 1 fps
    let output_pattern = temp_path.join("frame_%04d.png");
    let Some(output_pattern_str) = output_pattern.to_str() else {
        bail!("Invalid temp path");
    };

    ffmpeg::extract_frames(mkv_path, output_pattern_str)?;

    // Initialize OCR engine
    let api = create_ocr_engine()?;

    // Regex pattern for production code format:
    // - Seasons 1-5: #3X22 or #1X79 (season X episode)
    // - Season 6-9: #6ABX08 (season 6, episode 6) - format: #<season>ABX<episode>
    // - Season 10-11: #1AYW01, #2AYW01 - format: #<season>AYW<episode> (no X)
    // Case-insensitive, whitespace is stripped before matching
    // Matches: #<season>X<episode> or #<season><letters>X<episode> or #<season><letters><episode>
    let re = Regex::new(r"(?i)(?:\d[A-Z]{1,3}[\d]{2,3})|(?:1?\d[0-3]\d)")?;

    // Process extracted frames
    let mut frame_files: Vec<PathBuf> = fs::read_dir(temp_path)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()?.to_str()? == "png" {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    // Sort frames by name to process in order
    frame_files.sort();

    let mut candidates = Vec::new();
    // Try OCR on each frame until we find the production code
    for frame_path in frame_files {
        // Load image from file
        let img = match image::open(&frame_path) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("Warning: Failed to load image {frame_path:?}: {e}");
                continue;
            }
        };

        // Convert to RGB8 for tesseract (tesseract expects RGB)
        let rgb_img = img.to_rgb8();
        let (width, height) = rgb_img.dimensions();
        let image_data = rgb_img.into_raw();

        // Perform OCR
        match api.set_image(
            &image_data,
            width as i32,
            height as i32,
            3,                // bytes per pixel (RGB)
            3 * width as i32, // bytes per line
        ) {
            Ok(_) => {
                match api.get_utf8_text() {
                    Ok(text) => {
                        // Strip all whitespace from the text before matching
                        let text_no_whitespace: String = text
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .map(|c| match c {
                                'O' => '0',
                                'I' => '1',
                                'S' => '5',
                                '?' => 'X',
                                _ => c,
                            })
                            .collect();

                        // Search for production code pattern in the extracted text
                        let matches = re.find_iter(&text_no_whitespace);
                        for candidate in matches {
                            candidates.push(candidate.as_str().to_owned());
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to get OCR text from frame {frame_path:?}: {e}");
                        continue;
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to set image for OCR on frame {frame_path:?}: {e}");
                continue;
            }
        }
    }
    eprintln!("Found candidates: {candidates:?}");

    Ok(candidates)
}

/// Reads the text off the title card of a short: the frames of `duration_seconds` starting
/// `start_seconds` into `path`, one a second. Returns one text per frame and image preparation,
/// for matching against episode titles; the texts are noisy and mostly differ in what they get
/// wrong, so none of them is cleaned up here.
pub fn extract_title_card_text(
    path: &Path,
    start_seconds: u64,
    duration_seconds: u64,
) -> Result<Vec<String>> {
    let temp_dir = TempDir::new()?;
    let output_pattern = temp_dir.path().join("frame_%04d.png");
    ffmpeg::extract_frames_between(path, start_seconds, duration_seconds, &output_pattern)?;

    let mut frames: Vec<PathBuf> = fs::read_dir(temp_dir.path())?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect();
    frames.sort();
    if frames.is_empty() {
        bail!("No frames to read: is the file shorter than {start_seconds} seconds?");
    }

    let progress = ProgressBar::new(frames.len() as u64);
    progress.set_style(
        ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len} ({elapsed} elapsed)")
            .expect("valid progress template")
            .progress_chars("=> "),
    );
    progress.set_message("OCR of title card frames");

    // The bundled Tesseract is built without OpenMP, so one engine only ever uses one core. Each
    // worker owns an engine and drops it before it exits, which Tesseract needs: engines still
    // alive at process exit make its shared cache print leak warnings.
    let workers = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(frames.len());
    let next = AtomicUsize::new(0);
    let per_worker: Vec<Result<Vec<(usize, Vec<String>)>>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let api = create_ocr_engine()?;
                    // Title cards are a few words scattered over a picture, not a page of text
                    api.set_page_seg_mode(TessPageSegMode::PSM_SPARSE_TEXT)?;
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(frame) = frames.get(index) else {
                            break;
                        };
                        done.push((index, read_title_card_frame(&api, frame)));
                        progress.inc(1);
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
    progress.finish_and_clear();

    let mut by_frame = Vec::new();
    for worker in per_worker {
        by_frame.extend(worker?);
    }
    by_frame.sort_by_key(|(index, _)| *index);
    Ok(by_frame.into_iter().flat_map(|(_, texts)| texts).collect())
}

/// The text of one frame under each of `title_card_variants`, leaving out the blank ones.
fn read_title_card_frame(api: &TesseractAPI, frame_path: &Path) -> Vec<String> {
    let frame = match image::open(frame_path) {
        Ok(frame) => frame,
        Err(e) => {
            eprintln!("Warning: Failed to load image {frame_path:?}: {e}");
            return Vec::new();
        }
    };

    title_card_variants(&frame)
        .iter()
        .filter_map(|variant| {
            let rgb = variant.to_rgb8();
            let (width, height) = rgb.dimensions();
            api.set_image(
                &rgb.into_raw(),
                width as i32,
                height as i32,
                3,
                3 * width as i32,
            )
            .ok()?;
            api.get_utf8_text().ok()
        })
        .filter(|text| !text.trim().is_empty())
        .collect()
}

/// A frame's pixels as black text on white: pixels brighter (or, if `bright_is_text` is false,
/// darker) than `level` become black and the rest white.
fn threshold(frame: &DynamicImage, level: u8, bright_is_text: bool) -> DynamicImage {
    let mut gray = frame.to_luma8();
    for pixel in gray.pixels_mut() {
        let is_text = if bright_is_text {
            pixel.0[0] > level
        } else {
            pixel.0[0] < level
        };
        pixel.0[0] = if is_text { 0 } else { 255 };
    }
    DynamicImage::ImageLuma8(gray)
}

/// The ways each frame is prepared for OCR. Title card lettering is any colour, over any
/// background, light on dark or dark on light, so no single treatment reads them all; a title
/// is found if any one of these does. They were picked by trying many on the 24 title cards of
/// a Tom and Jerry disc: these four together read as many titles as all of the others combined.
fn title_card_variants(frame: &DynamicImage) -> Vec<DynamicImage> {
    let (width, height) = (frame.width(), frame.height());
    let double = frame.resize(width * 2, height * 2, FilterType::Lanczos3);
    let triple = frame.resize(width * 3, height * 3, FilterType::Lanczos3);
    vec![
        threshold(&triple, 110, true),
        triple.grayscale(),
        threshold(&triple, 110, false),
        threshold(&double, 140, true),
    ]
}

pub fn create_ocr_engine() -> Result<TesseractAPI> {
    let api = TesseractAPI::new();
    api.init_5(ENG, ENG.len() as i32, "eng", 3, &[])?;

    Ok(api)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_production_code_regex() {
        // This test verifies the regex logic used in the main function.
        // Ideally the regex should be a shared constant or returned by a function,
        // but since we can't refactor, we duplicate it here to verify the logic described in comments.
        let re = Regex::new(r"(?i)(?:\d[A-Z]{1,3}[\d]{2,3})|(?:1?\d[0-3]\d)").unwrap();

        let valid_cases = vec![
            "3X22", // Season 1-5 format
            "1X79", "6ABX08", // Season 6-9 format
            "1AYW01", // Season 10-11 format
            "2AYW01", "3x22", // Lowercase
            "6abx08", "1030", // Numeric format (example from second branch of regex)
            "912",
        ];

        for case in valid_cases {
            assert!(re.is_match(case), "Failed to match valid case: {}", case);
        }
    }

    #[test]
    fn test_ocr_text_cleaning_logic() {
        // Testing the character replacement logic
        let input = "S 0 I ? O";
        let expected = "501X0"; // S->5, space removed, I->1, ?->X, O->0

        let cleaned: String = input
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match c {
                'O' => '0',
                'I' => '1',
                'S' => '5',
                '?' => 'X',
                _ => c,
            })
            .collect();

        assert_eq!(cleaned, expected);
    }
}
