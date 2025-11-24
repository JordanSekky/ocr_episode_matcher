use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use regex::Regex;
use rten::Model;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

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
            bail!("Failed to execute ffmpeg: {}", e);
        }
    };

    if !ffmpeg_output.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
        bail!("FFmpeg error: {}", stderr);
    }

    // Initialize OCR engine with models
    let ocr_engine = create_ocr_engine()?;

    // Regex pattern for production code format:
    // - Seasons 1-5: #3X22 or #1X79 (season X episode)
    // - Season 6-9: #6ABX08 (season 6, episode 6) - format: #<season>ABX<episode>
    // - Season 10-11: #1AYW01, #2AYW01 - format: #<season>AYW<episode> (no X)
    // Case-insensitive, whitespace is stripped before matching
    // Matches: #<season>X<episode> or #<season><letters>X<episode> or #<season><letters><episode>
    let re = Regex::new(r"(?i)\d[A-Z]+[\dO]{2,3}")?;

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
                eprintln!("Warning: Failed to load image {:?}: {}", frame_path, e);
                continue;
            }
        };

        // Convert to grayscale for OCR
        let gray_img = img.to_luma8();
        let (width, height) = gray_img.dimensions();

        // Get image bytes
        let image_bytes = gray_img.as_raw();

        // Create image source for OCR using from_bytes
        let image_source = match ImageSource::from_bytes(image_bytes, (width, height)) {
            Ok(source) => source,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to create image source {:?}: {}",
                    frame_path, e
                );
                continue;
            }
        };

        // Prepare OCR input (takes ImageSource by value, not reference)
        let ocr_input = match ocr_engine.prepare_input(image_source) {
            Ok(input) => input,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to prepare OCR input {:?}: {}",
                    frame_path, e
                );
                continue;
            }
        };

        // Perform OCR using convenience method that extracts all text
        match ocr_engine.get_text(&ocr_input) {
            Ok(text) => {
                // Strip all whitespace from the text before matching
                let text_no_whitespace: String =
                    text.chars().filter(|c| !c.is_whitespace()).collect();

                // Search for production code pattern in the extracted text
                let matches = re.find_iter(&text_no_whitespace);
                for candidate in matches {
                    candidates.push(candidate.as_str().to_owned());
                }
            }
            Err(e) => {
                // Continue to next frame if OCR fails on this one
                eprintln!("Warning: OCR failed on frame {:?}: {}", frame_path, e);
                continue;
            }
        }
    }

    Ok(candidates)
}

fn create_ocr_engine() -> Result<OcrEngine> {
    const DETECTION_MODEL_URL: &str =
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten";
    const RECOGNITION_MODEL_URL: &str =
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten";

    // Determine model directory - use $HOME/.episode-matcher/
    let model_dir = env::var("HOME")
        .map(|home| PathBuf::from(home).join(".episode-matcher"))
        .unwrap_or_else(|_| PathBuf::from(".episode-matcher"));

    // Create model directory if it doesn't exist
    if !model_dir.exists() {
        fs::create_dir_all(&model_dir)?;
    }

    let detection_model_path = model_dir.join("text-detection.rten");
    let recognition_model_path = model_dir.join("text-recognition.rten");

    // Check environment variables first
    let detection_model_path = env::var("OCRS_DETECTION_MODEL")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or(detection_model_path);

    let recognition_model_path = env::var("OCRS_RECOGNITION_MODEL")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or(recognition_model_path);

    // Download detection model if it doesn't exist
    if !detection_model_path.exists() {
        eprintln!(
            "Downloading detection model to {:?}...",
            detection_model_path
        );
        download_file(DETECTION_MODEL_URL, &detection_model_path)?;
    }

    // Download recognition model if it doesn't exist
    if !recognition_model_path.exists() {
        eprintln!(
            "Downloading recognition model to {:?}...",
            recognition_model_path
        );
        download_file(RECOGNITION_MODEL_URL, &recognition_model_path)?;
    }

    // Load models
    let detection_model = Model::load_file(&detection_model_path).map_err(|e| {
        anyhow!(
            "Failed to load detection model from {:?}: {}",
            detection_model_path,
            e
        )
    })?;

    let recognition_model = Model::load_file(&recognition_model_path).map_err(|e| {
        anyhow!(
            "Failed to load recognition model from {:?}: {}",
            recognition_model_path,
            e
        )
    })?;

    Ok(OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })?)
}

fn download_file(url: &str, path: &Path) -> Result<()> {
    let response = reqwest::blocking::get(url)?;
    if !response.status().is_success() {
        bail!("Failed to download {}: HTTP {}", url, response.status());
    }

    let mut file = fs::File::create(path)?;
    let content = response.bytes()?;
    file.write_all(&content)?;

    Ok(())
}
