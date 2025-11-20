mod config;
mod ocr;
mod rename;
mod tvdb;

use clap::Parser;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tvdb::TvdbClient;

#[derive(Parser)]
#[command(name = "episode-matcher")]
#[command(about = "Extract production codes from video files and rename them using TVDB data")]
struct Cli {
    /// Input file or directory
    #[arg(short = 'i', long = "input", required = true)]
    input: PathBuf,

    /// Show name to search in TVDB
    #[arg(long)]
    show: Option<String>,

    /// Direct TVDB show ID
    #[arg(long)]
    show_id: Option<String>,

    /// Skip confirmation prompts
    #[arg(long)]
    no_confirm: bool,
}

fn main() {
    let cli = Cli::parse();

    // Validate input path exists
    if !cli.input.exists() {
        eprintln!("Error: Input path does not exist: {:?}", cli.input);
        std::process::exit(1);
    }

    // Get TVDB API key
    let api_key = match config::get_tvdb_api_key() {
        Ok(key) => key,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // Determine show ID
    let show_id = match (cli.show, cli.show_id) {
        (Some(show_name), None) => match search_and_select_show(&api_key, &show_name) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("Error searching for show: {}", e);
                std::process::exit(1);
            }
        },
        (None, Some(id)) => id,
        (Some(_), Some(_)) => {
            eprintln!("Error: Cannot specify both --show and --show-id");
            std::process::exit(1);
        }
        (None, None) => {
            eprintln!("Error: Must specify either --show or --show-id");
            std::process::exit(1);
        }
    };

    // Process input (file or directory)
    if cli.input.is_file() {
        if let Err(e) = process_file(&cli.input, &api_key, &show_id, cli.no_confirm) {
            eprintln!("Error processing file: {}", e);
            std::process::exit(1);
        }
    } else if cli.input.is_dir() {
        if let Err(e) = process_directory(&cli.input, &api_key, &show_id, cli.no_confirm) {
            eprintln!("Error processing directory: {}", e);
            std::process::exit(1);
        }
    } else {
        eprintln!("Error: Input path is neither a file nor a directory");
        std::process::exit(1);
    }
}

fn search_and_select_show(
    api_key: &str,
    query: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut client = TvdbClient::new(api_key.to_string());
    client.login()?;

    let results = client.search_series(query)?;

    if results.is_empty() {
        return Err(format!("No shows found matching '{}'", query).into());
    }

    if results.len() == 1 {
        return Ok(results[0].tvdb_id.clone());
    }

    // Multiple results - let user select
    println!("Multiple shows found. Please select one:");
    for (i, result) in results.iter().enumerate() {
        let name = result
            .name
            .as_ref()
            .and_then(|n| n.get("eng"))
            .or_else(|| result.name.as_ref().and_then(|n| n.values().next()))
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        println!("  {}: {} (ID: {})", i + 1, name, result.tvdb_id);
    }

    print!("Enter number (1-{}): ", results.len());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let choice: usize = input.trim().parse().map_err(|_| "Invalid selection")?;

    if choice < 1 || choice > results.len() {
        return Err("Invalid selection".into());
    }

    Ok(results[choice - 1].tvdb_id.clone())
}

fn process_file(
    file_path: &Path,
    api_key: &str,
    show_id: &str,
    skip_confirm: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if file_path.extension().and_then(|s| s.to_str()) != Some("mkv") {
        eprintln!("Skipping non-MKV file: {:?}", file_path);
        return Ok(());
    }

    println!("Processing: {:?}", file_path);

    // Extract production code
    let production_code = match ocr::extract_production_code(file_path.to_str().unwrap()) {
        Ok(Some(code)) => code,
        Ok(None) => {
            eprintln!(
                "Warning: Could not extract production code from {:?}",
                file_path
            );
            return Ok(());
        }
        Err(e) => {
            eprintln!("Error extracting production code: {}", e);
            return Ok(());
        }
    };

    println!("Found production code: {}", production_code);

    // Lookup episode in TVDB
    let mut client = TvdbClient::new(api_key.to_string());
    client.login()?;

    let episode = match client.find_episode_by_production_code(show_id, &production_code)? {
        Some(ep) => ep,
        None => {
            eprintln!(
                "Warning: Episode not found in TVDB for production code: {}",
                production_code
            );
            return Ok(());
        }
    };

    // Get show name
    let show_name = client.get_series_name(show_id)?;

    // Generate new filename
    let new_filename = rename::generate_filename(
        &show_name,
        episode.season_number,
        episode.episode_number,
        &episode.name,
    );

    // Find unique filename if needed
    let directory = file_path.parent().unwrap_or(Path::new("."));
    let new_path = rename::find_unique_filename(directory, &new_filename);

    // Rename file
    rename::rename_file(file_path, &new_path, skip_confirm)?;

    Ok(())
}

fn process_directory(
    dir_path: &Path,
    api_key: &str,
    show_id: &str,
    skip_confirm: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = fs::read_dir(dir_path)?;
    let mut mkv_files: Vec<PathBuf> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_file() && path.extension()?.to_str()? == "mkv" {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    mkv_files.sort();

    println!("Found {} MKV file(s) to process", mkv_files.len());

    for file_path in mkv_files {
        if let Err(e) = process_file(&file_path, api_key, show_id, skip_confirm) {
            eprintln!("Error processing {:?}: {}", file_path, e);
            // Continue processing other files
        }
        println!(); // Blank line between files
    }

    Ok(())
}
