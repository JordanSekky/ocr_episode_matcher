mod cache;
mod config;
mod ocr;
mod rename;
mod tvdb;

use anyhow::{bail, Result};
use cache::Cache;
use clap::Parser;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tvdb::TvdbClient;

#[derive(Parser)]
#[command(name = "episode-matcher")]
#[command(about = "Extract production codes from video files and rename them using TVDB data")]
struct Cli {
    /// Input files or directories to process
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Show name to search in TVDB
    #[arg(long)]
    show: Option<String>,

    /// Direct TVDB show ID
    #[arg(long)]
    show_id: Option<String>,

    /// Skip confirmation prompts
    #[arg(long)]
    no_confirm: bool,

    /// Recursively scan directories for MKV files
    #[arg(short = 'r', long = "recursive")]
    recursive: bool,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    // Get TVDB API key
    let api_key = config::get_tvdb_api_key()?;

    // Load cache
    let mut cache = Cache::load();
    let mut client = TvdbClient::new(api_key.to_string());

    // Determine show ID
    let show_id = match (cli.show, cli.show_id) {
        (Some(show_name), None) => match search_and_select_show(&mut client, &show_name) {
            Ok(id) => id,
            Err(e) => {
                bail!("Error searching for show: {}", e);
            }
        },
        (None, Some(id)) => id,
        (Some(_), Some(_)) => {
            bail!("Error: Cannot specify both --show and --show-id");
        }
        (None, None) => {
            bail!("Error: Must specify either --show or --show-id");
        }
    };

    // Preload cache with series name and all episodes (only if not already cached)
    if !cache.has_series_episodes(&show_id) {
        if let Err(e) = preload_cache(&mut client, &show_id, &mut cache) {
            eprintln!("Warning: Failed to preload cache: {}", e);
        } else {
            // Save cache after preloading
            if let Err(e) = cache.save() {
                eprintln!("Warning: Failed to save cache after preload: {}", e);
            }
        }
    } else {
        println!("Using cached episode data for series {}", show_id);
    }

    // Get show name from cache or API
    let show_name = match get_show_name(&mut client, &show_id, &mut cache) {
        Ok(name) => name,
        Err(e) => {
            bail!("Error getting show name: {}", e);
        }
    };

    // Validate and process all input paths
    for input_path in &cli.inputs {
        if !input_path.exists() {
            eprintln!("Error: Input path does not exist: {:?}", input_path);
            continue;
        }

        if let Err(e) = process_input_path(
            input_path,
            &show_id,
            &show_name,
            cli.no_confirm,
            cli.recursive,
            &mut client,
            &mut cache,
        ) {
            eprintln!("Error processing path {:?}: {}", input_path, e);
            // Continue processing other paths
        }
    }

    // Save cache before exiting
    if let Err(e) = cache.save() {
        eprintln!("Warning: Failed to save cache: {}", e);
    }

    Ok(())
}

fn process_input_path(
    input_path: &Path,
    show_id: &str,
    show_name: &str,
    skip_confirm: bool,
    recursive: bool,
    client: &mut TvdbClient,
    cache: &mut Cache,
) -> Result<()> {
    if input_path.is_file() {
        process_file(input_path, show_id, show_name, skip_confirm, client, cache)?;
    } else if input_path.is_dir() {
        process_directory(
            input_path,
            show_id,
            show_name,
            skip_confirm,
            recursive,
            client,
            cache,
        )?;
    } else {
        bail!("Input path is neither a file nor a directory");
    }

    Ok(())
}

fn preload_cache(client: &mut TvdbClient, series_id: &str, cache: &mut Cache) -> Result<()> {
    // Get series name if not cached
    if cache.get_series_name(series_id).is_none() {
        let series_name = client.get_series_name(series_id)?;
        cache.set_series_name(series_id.to_string(), series_name);
    }

    // Preload all episodes for this series
    println!("Preloading episode cache for series {}...", series_id);
    client.preload_episodes(series_id, cache)?;
    println!("Cache preloaded successfully.");

    Ok(())
}

fn search_and_select_show(client: &mut TvdbClient, query: &str) -> Result<String> {
    let results = client.search_series(query)?;

    if results.is_empty() {
        bail!("No shows found matching '{}'", query);
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
    let choice: usize = input
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid selection"))?;

    if choice < 1 || choice > results.len() {
        bail!("Invalid selection");
    }

    Ok(results[choice - 1].tvdb_id.clone())
}

fn process_file(
    file_path: &Path,
    show_id: &str,
    show_name: &str,
    skip_confirm: bool,
    client: &mut TvdbClient,
    cache: &mut Cache,
) -> Result<()> {
    if file_path.extension().and_then(|s| s.to_str()) != Some("mkv") {
        bail!("Skipping non-MKV file: {:?}", file_path);
    }

    println!("Processing: {:?}", file_path);

    // Extract production code
    let production_code_candidates =
        ocr::extract_production_code_candidates(file_path.to_str().unwrap())?;
    if production_code_candidates.is_empty() {
        eprintln!(
            "Warning: No production code candidates found for {:?}",
            file_path
        );
        return Ok(());
    }

    let Some(episode) = production_code_candidates.into_iter().find_map(|code| {
        client
            .find_episode_by_production_code(show_id, &code, cache)
            .ok()
            .flatten()
    }) else {
        eprintln!(
            "Warning: No production code found in cache for {:?}",
            file_path
        );
        return Ok(());
    };

    println!(
        "Found episode: S{}E{} - {}",
        episode.season_number, episode.episode_number, episode.name
    );

    // Generate new filename
    let new_filename = rename::generate_filename(
        show_name,
        episode.season_number,
        episode.episode_number,
        &episode.name,
    );

    // Find unique filename if needed
    let directory = file_path.parent().unwrap_or(Path::new("."));
    let new_path = rename::find_unique_filename(file_path, directory, &new_filename);

    // Rename file
    rename::rename_file(file_path, &new_path, skip_confirm)?;

    Ok(())
}

fn process_directory(
    dir_path: &Path,
    show_id: &str,
    show_name: &str,
    skip_confirm: bool,
    recursive: bool,
    client: &mut TvdbClient,
    cache: &mut Cache,
) -> Result<()> {
    let mkv_files = if recursive {
        collect_mkv_files_recursive(dir_path)?
    } else {
        collect_mkv_files(dir_path)?
    };

    println!("Found {} MKV file(s) to process", mkv_files.len());

    for file_path in mkv_files {
        if let Err(e) = process_file(&file_path, show_id, show_name, skip_confirm, client, cache) {
            eprintln!("Error processing {:?}: {}", file_path, e);
            // Continue processing other files
        }
        println!(); // Blank line between files
    }

    Ok(())
}

fn collect_mkv_files(dir_path: &Path) -> Result<Vec<PathBuf>> {
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
    Ok(mkv_files)
}

fn collect_mkv_files_recursive(dir_path: &Path) -> Result<Vec<PathBuf>> {
    let mut mkv_files = Vec::new();
    collect_mkv_files_recursive_helper(dir_path, &mut mkv_files)?;
    mkv_files.sort();
    Ok(mkv_files)
}

fn collect_mkv_files_recursive_helper(dir_path: &Path, mkv_files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir_path)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("mkv") {
                mkv_files.push(path);
            }
        } else if path.is_dir() {
            // Recursively scan subdirectories
            collect_mkv_files_recursive_helper(&path, mkv_files)?;
        }
    }

    Ok(())
}

fn get_show_name(client: &mut TvdbClient, show_id: &str, cache: &mut Cache) -> Result<String> {
    if let Some(name) = cache.get_series_name(show_id) {
        return Ok(name.clone());
    }
    let name = client.get_series_name(show_id)?;
    cache.set_series_name(show_id.to_string(), name.clone());
    Ok(name)
}
