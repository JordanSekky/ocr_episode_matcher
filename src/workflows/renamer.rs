use rustyline::DefaultEditor;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

pub fn generate_filename(
    show_name: &str,
    season: u64,
    episode: u64,
    episode_title: &str,
) -> String {
    format!(
        "{} - S{:02}E{:02} - {}.mkv",
        sanitize_filename(show_name),
        season,
        episode,
        sanitize_filename(episode_title)
    )
}

fn sanitize_filename(name: &str) -> String {
    // Remove or replace invalid filename characters
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn find_unique_filename(old_path: &Path, directory: &Path, base_filename: &str) -> PathBuf {
    let mut path = directory.join(base_filename);
    let mut counter = 1;

    while path.exists() && path.to_string_lossy() != old_path.to_string_lossy() {
        let stem = Path::new(base_filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let extension = Path::new(base_filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("mkv");

        let new_filename = format!("{stem} [copy {counter}].{extension}");
        path = directory.join(new_filename);
        counter += 1;
    }

    path
}

pub fn confirm_rename(old_path: &Path, new_path: &Path) -> bool {
    println!(
        "Rename \"{}\" -> \"{}\"? [y/N] ",
        old_path.file_name().unwrap().to_string_lossy(),
        new_path.file_name().unwrap().to_string_lossy()
    );

    let mut rl = DefaultEditor::new().unwrap();
    let input = match rl.readline("") {
        Ok(line) => line,
        Err(_) => String::new(),
    };

    input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes"
}

pub fn rename_file(old_path: &Path, new_path: &Path, skip_confirm: bool) -> Result<()> {
    if old_path.to_string_lossy() == new_path.to_string_lossy() {
        println!("File is already named correctly.");
        return Ok(());
    }
    if !skip_confirm && !confirm_rename(old_path, new_path) {
        println!("Skipped.");
        return Ok(());
    }

    fs::rename(old_path, new_path)?;
    println!("Renamed successfully.");
    Ok(())
}
