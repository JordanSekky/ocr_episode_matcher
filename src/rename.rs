use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn generate_filename(
    show_name: &str,
    season: u32,
    episode: u32,
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

pub fn find_unique_filename(directory: &Path, base_filename: &str) -> PathBuf {
    let mut path = directory.join(base_filename);
    let mut counter = 1;

    while path.exists() {
        let stem = Path::new(base_filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let extension = Path::new(base_filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("mkv");

        let new_filename = format!("{} ({}).{}", stem, counter, extension);
        path = directory.join(new_filename);
        counter += 1;
    }

    path
}

pub fn confirm_rename(old_path: &Path, new_path: &Path) -> bool {
    print!(
        "Rename \"{}\" -> \"{}\"? [y/N] ",
        old_path.file_name().unwrap().to_string_lossy(),
        new_path.file_name().unwrap().to_string_lossy()
    );
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes"
}

pub fn rename_file(
    old_path: &Path,
    new_path: &Path,
    skip_confirm: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !skip_confirm && !confirm_rename(old_path, new_path) {
        println!("Skipped.");
        return Ok(());
    }

    fs::rename(old_path, new_path)?;
    println!("Renamed successfully.");
    Ok(())
}
