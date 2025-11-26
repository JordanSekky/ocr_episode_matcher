use anyhow::bail;
use anyhow::Result;
use regex::Regex;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use tesseract_rs::TesseractAPI;

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

    let ffmpeg_output = Command::new("ffmpeg")
        .arg("-sseof")
        .arg("-15")
        .arg("-i")
        .arg(mkv_path)
        .arg("-vf")
        .arg("fps=1")
        .arg("-y")
        .arg(output_pattern_str)
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

fn create_ocr_engine() -> Result<TesseractAPI> {
    let api = TesseractAPI::new();
    // Initialize with tessdata directory and English language
    api.init_5(ENG, ENG.len() as i32, "eng", 3, &[])?;

    Ok(api)
}
