mod cli;
mod config;
mod domain;
mod infra;
mod media;
mod workflows;

use anyhow::{bail, Result};
use clap::Parser;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cli::Cli;
use domain::models::EpisodeEntry;
use infra::cache::Cache;
use infra::ollama::OllamaClient;
use infra::tvdb::TvdbClient;
use workflows::matchers::{
    prod_code::ProductionCodeMatcher, subtitle::SubtitleMatcher, EpisodeMatch, MatchContext,
    Matcher, RipContext,
};
use workflows::renamer;

use crate::cli::MatchMode;

/// What stays the same for every file in a run.
struct Session<'a> {
    series_id: &'a str,
    show_name: &'a str,
    skip_confirm: bool,
    hint: Option<(u64, u64)>,
    matcher: &'a dyn Matcher,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    // Get TVDB API key
    let api_key = config::get_tvdb_api_key()?;

    // Ollama is optional and only used for subtitle matching
    let ollama = match config::get_ollama_config()? {
        Some(config) if cli.match_mode == MatchMode::Subtitles => {
            println!("Using Ollama model {} at {}", config.model, config.url);
            Some(OllamaClient::new(config)?)
        }
        _ => None,
    };

    // Load cache
    let mut cache = Cache::load();
    let mut client = TvdbClient::new(api_key.to_string());

    // Determine show ID
    let show_id = match (cli.show, cli.show_id) {
        (Some(show_name), None) => match search_and_select_show(&mut client, &show_name) {
            Ok(id) => id,
            Err(e) => {
                bail!("Error searching for show: {e}");
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

    // Preload cache with series name and all episodes (only if not already cached). Caches from
    // before overviews and runtimes were stored are refreshed when Ollama needs them.
    let mut needs_preload = !cache.has_series_episodes(&show_id);
    if !needs_preload && ollama.is_some() && !cache.is_series_detailed(&show_id) {
        println!("Refreshing cache to fetch episode overviews and runtimes...");
        needs_preload = true;
    }
    if needs_preload {
        preload_cache(&mut client, &show_id, &mut cache)?;
        // Persist right away: the preload is slow, and bailing or being interrupted later in the
        // run would otherwise throw it away and force a refetch next time.
        if let Err(e) = cache.save() {
            eprintln!("Warning: Failed to save cache: {e}");
        }
    } else {
        println!("Using cached episode data for series {show_id}");
    }

    // Get show name from cache or API
    let show_name = match get_show_name(&mut client, &show_id, &mut cache) {
        Ok(name) => name,
        Err(e) => {
            bail!("Error getting show name: {e}");
        }
    };

    if let Some((season, episode)) = cli.hint {
        if ollama.is_none() {
            eprintln!("Warning: --hint only affects Ollama matching and will be ignored.");
        } else if cache
            .get_episode_by_sxxexx(&show_id, season, episode)
            .is_none()
        {
            bail!("--hint S{season:02}E{episode:02} is not an episode of this series");
        }
    }

    let matcher: Box<dyn Matcher> = match cli.match_mode {
        MatchMode::ProductionCode => Box::new(ProductionCodeMatcher {
            prompt_size: cli.prompt_size,
        }),
        MatchMode::Subtitles => Box::new(SubtitleMatcher {
            start_seconds: cli.subtitle_start,
            duration_seconds: cli.subtitle_duration,
            prefer_codec: cli.prefer_codec,
            skip_confirm: cli.no_confirm,
            include_specials: cli.include_specials,
            ollama,
        }),
    };

    let session = Session {
        series_id: &show_id,
        show_name: &show_name,
        skip_confirm: cli.no_confirm,
        hint: cli.hint,
        matcher: matcher.as_ref(),
    };

    // Validate and process all input paths
    for input_path in &cli.inputs {
        if !input_path.exists() {
            eprintln!("Error: Input path does not exist: {input_path:?}");
            continue;
        }

        if let Err(e) = process_input_path(input_path, &session, cli.recursive, &mut cache) {
            eprintln!("Error processing path {input_path:?}: {e}");
            // Continue processing other paths
        }
    }

    // Save cache before exiting
    if let Err(e) = cache.save() {
        eprintln!("Warning: Failed to save cache: {e}");
    }

    Ok(())
}

fn process_input_path(
    input_path: &Path,
    session: &Session,
    recursive: bool,
    cache: &mut Cache,
) -> Result<()> {
    if input_path.is_file() {
        let rip = RipContext::new(vec![input_path.to_path_buf()]).with_hint(session.hint);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: session.show_name,
        };
        process_file(input_path, session, cache, &ctx)?;
    } else if input_path.is_dir() {
        process_directory(input_path, session, recursive, cache)?;
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
    println!("Preloading episode cache for series {series_id}...");
    client.preload_episodes(series_id, cache)?;
    println!("Cache preloaded successfully.");

    Ok(())
}

fn search_and_select_show(client: &mut TvdbClient, query: &str) -> Result<String> {
    let results = client.search_series(query)?;

    if results.is_empty() {
        bail!("No shows found matching '{query}'");
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

    loop {
        print!("Enter number (1-{}): ", results.len());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let choice: usize = match input.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                println!("Invalid input. Please enter a number.");
                continue;
            }
        };

        if choice >= 1 && choice <= results.len() {
            return Ok(results[choice - 1].tvdb_id.clone());
        }
        println!(
            "Invalid selection. Please enter a number between 1 and {}.",
            results.len()
        );
    }
}

fn process_file(
    file_path: &Path,
    session: &Session,
    cache: &mut Cache,
    ctx: &MatchContext,
) -> Result<Option<EpisodeEntry>> {
    if file_path.extension().and_then(|s| s.to_str()) != Some("mkv") {
        bail!("Skipping non-MKV file: {file_path:?}");
    }

    println!("Processing: {file_path:?}");

    let matched = session
        .matcher
        .match_episode(file_path, session.series_id, cache, ctx)?;

    let Some(EpisodeMatch { episode, confirmed }) = matched else {
        eprintln!("Warning: No matching episode found for {file_path:?}");
        return Ok(None);
    };

    println!(
        "Found episode: S{}E{} - {}",
        episode.season_number, episode.episode_number, episode.name
    );

    // Generate new filename
    let new_filename = renamer::generate_filename(
        session.show_name,
        episode.season_number,
        episode.episode_number,
        &episode.name,
    );

    // Find unique filename if needed
    let directory = file_path.parent().unwrap_or(Path::new("."));
    let new_path = renamer::find_unique_filename(file_path, directory, &new_filename);

    // Accepting the match at the prompt already approved the rename
    let applied = renamer::rename_file(file_path, &new_path, session.skip_confirm || confirmed)?;

    // Only confirmed matches count as locality hints for the following files
    Ok(applied.then_some(episode))
}

fn process_directory(
    dir_path: &Path,
    session: &Session,
    recursive: bool,
    cache: &mut Cache,
) -> Result<()> {
    let mkv_files = collect_mkv_files(dir_path, recursive)?;

    println!("Found {} MKV file(s) to process", mkv_files.len());

    // Files sorted alphabetically are usually adjacent episodes, so matches found so far are
    // passed along as hints for the files that follow.
    let mut rip = RipContext::new(mkv_files.clone()).with_hint(session.hint);

    for (index, file_path) in mkv_files.iter().enumerate() {
        let ctx = MatchContext {
            rip: &rip,
            index,
            show_name: session.show_name,
        };
        match process_file(file_path, session, cache, &ctx) {
            Ok(matched) => rip.matches[index] = matched,
            Err(e) => {
                eprintln!("Error processing {file_path:?}: {e}");
                // Continue processing other files
            }
        }
        println!(); // Blank line between files
    }

    Ok(())
}

fn collect_mkv_files(dir_path: &Path, recurse: bool) -> Result<Vec<PathBuf>> {
    let mut mkv_files = Vec::new();
    collect_mkv_files_helper(dir_path, recurse, &mut mkv_files)?;
    mkv_files.sort();
    Ok(mkv_files)
}

fn collect_mkv_files_helper(
    dir_path: &Path,
    recurse: bool,
    mkv_files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(dir_path)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("mkv") {
                mkv_files.push(path);
            }
        } else if path.is_dir() && recurse {
            // Recursively scan subdirectories
            collect_mkv_files_helper(&path, recurse, mkv_files)?;
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
