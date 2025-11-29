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
    loop {
        let input = rl.readline("").unwrap_or_default();
        let input = input.trim().to_lowercase();

        if input == "y" || input == "yes" {
            return true;
        } else if input == "n" || input == "no" || input.is_empty() {
            return false;
        } else {
            println!("Please enter 'y' or 'n'.");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("Normal Name"), "Normal Name");
        assert_eq!(sanitize_filename("Name/With/Slashes"), "Name-With-Slashes");
        assert_eq!(
            sanitize_filename("Name\\With\\Backslashes"),
            "Name-With-Backslashes"
        );
        assert_eq!(sanitize_filename("Name:With:Colons"), "Name-With-Colons");
        assert_eq!(sanitize_filename("Name*With*Stars"), "Name-With-Stars");
        assert_eq!(
            sanitize_filename("Name?With?Questions"),
            "Name-With-Questions"
        );
        assert_eq!(sanitize_filename("Name\"With\"Quotes"), "Name-With-Quotes");
        assert_eq!(sanitize_filename("Name<With<Less"), "Name-With-Less");
        assert_eq!(sanitize_filename("Name>With>Greater"), "Name-With-Greater");
        assert_eq!(sanitize_filename("Name|With|Pipes"), "Name-With-Pipes");
        assert_eq!(sanitize_filename("  Trim Me  "), "Trim Me");
    }

    #[test]
    fn test_generate_filename() {
        assert_eq!(
            generate_filename("Show Name", 1, 1, "Episode Name"),
            "Show Name - S01E01 - Episode Name.mkv"
        );
        assert_eq!(
            generate_filename("Show: Name", 2, 15, "Ep/isode?"),
            "Show- Name - S02E15 - Ep-isode-.mkv"
        );
    }

    #[test]
    fn test_find_unique_filename_no_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let old_path = dir_path.join("original.mkv");
        let base_filename = "Show - S01E01 - Episode.mkv";

        let unique_path = find_unique_filename(&old_path, dir_path, base_filename);
        assert_eq!(unique_path, dir_path.join(base_filename));
    }

    #[test]
    fn test_find_unique_filename_with_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let old_path = dir_path.join("original.mkv");
        let base_filename = "Show - S01E01 - Episode.mkv";

        // Create the conflicting file
        File::create(dir_path.join(base_filename)).unwrap();

        let unique_path = find_unique_filename(&old_path, dir_path, base_filename);
        assert_eq!(
            unique_path,
            dir_path.join("Show - S01E01 - Episode [copy 1].mkv")
        );

        // Create the first copy conflict
        File::create(dir_path.join("Show - S01E01 - Episode [copy 1].mkv")).unwrap();
        let unique_path_2 = find_unique_filename(&old_path, dir_path, base_filename);
        assert_eq!(
            unique_path_2,
            dir_path.join("Show - S01E01 - Episode [copy 2].mkv")
        );
    }

    #[test]
    fn test_find_unique_filename_same_file() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let filename = "Show - S01E01 - Episode.mkv";
        let old_path = dir_path.join(filename);

        // Even if the file exists, if it's the *same* file we are renaming (same path),
        // it should return that path (meaning no actual rename needed or it's overwrite safe in logic terms,
        // though fs::rename allows overwrite. The function check says `path.to_string_lossy() != old_path.to_string_lossy()`).
        // Wait, find_unique_filename logic:
        // while path.exists() && path != old_path
        // So if path == old_path, the loop condition fails immediately, returning path.
        // This handles the case where we are renaming a file to its current name (no-op).

        File::create(&old_path).unwrap();

        let unique_path = find_unique_filename(&old_path, dir_path, filename);
        assert_eq!(unique_path, old_path);
    }
}
