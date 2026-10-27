use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::media::ffmpeg;

/// Width that numbers up to `total` are padded to, at least two digits, so the pieces sort in
/// order.
fn number_width(total: usize) -> usize {
    total.to_string().len().max(2)
}

/// `<stem>_ch<label>.<extension>` next to `source`. `escape_stem` doubles any `%` in the stem so
/// ffmpeg, which reads a `%` in an output path as a number to fill in, takes it literally.
fn chapter_path(source: &Path, label: &str, escape_stem: bool) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let stem = if escape_stem {
        stem.replace('%', "%%")
    } else {
        stem.to_string()
    };
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("mkv");
    source.with_file_name(format!("{stem}_ch{label}.{extension}"))
}

/// Splits `source` into one file per chapter, next to it, and returns the new files in chapter
/// order. The source is left alone. Nothing is written if any of the files already exists.
pub fn split_by_chapter(source: &Path) -> Result<Vec<PathBuf>> {
    let chapters = ffmpeg::get_chapters(source)?;
    if chapters.len() < 2 {
        bail!(
            "{source:?} has {} chapter(s), so there is nothing to split",
            chapters.len()
        );
    }

    let total = chapters.len();
    let width = number_width(total);
    let outputs: Vec<PathBuf> = (1..=total)
        .map(|n| chapter_path(source, &format!("{n:0width$}"), false))
        .collect();
    if let Some(existing) = outputs.iter().find(|p| p.exists()) {
        bail!("{existing:?} already exists; move it aside to split {source:?} again");
    }

    let pattern = chapter_path(source, &format!("%0{width}d"), true);

    let message = format!("Splitting into {total} chapters with ffmpeg");
    if let Err(e) = ffmpeg::split_at_chapters(source, &chapters, &pattern, message) {
        // Don't leave truncated pieces that look like finished ones
        for output in &outputs {
            let _ = fs::remove_file(output);
        }
        return Err(e);
    }

    for output in &outputs {
        println!("Wrote {output:?}");
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_path_goes_next_to_the_source() {
        let path = chapter_path(Path::new("/rips/E1_t00.mkv"), "03", false);
        assert_eq!(path, Path::new("/rips/E1_t00_ch03.mkv"));
    }

    #[test]
    fn chapter_path_escapes_percent_for_ffmpeg() {
        let path = chapter_path(Path::new("100%.mkv"), "%02d", true);
        assert_eq!(path, Path::new("100%%_ch%02d.mkv"));
    }

    #[test]
    fn number_width_is_two_digits_until_100_chapters() {
        assert_eq!(number_width(24), 2);
        assert_eq!(number_width(99), 2);
        assert_eq!(number_width(120), 3);
    }
}
