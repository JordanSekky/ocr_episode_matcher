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

use cli::{Cli, Command, RenameArgs, SplitArgs};
use infra::cache::Cache;
use infra::jev::JevClient;
use infra::metadata::{cache_key, parse_show_id, MetadataProvider, Provider};
use infra::tmdb::TmdbClient;
use infra::tvdb::TvdbClient;
use workflows::matchers::{
    prod_code::ProductionCodeMatcher, subtitle::SubtitleMatcher, title_card::TitleCardMatcher,
    EpisodeMatch, MatchContext, Matcher,
};
use workflows::{renamer, splitter};

use crate::cli::MatchMode;

/// What stays the same for every file in a run.
struct Session<'a> {
    series_id: &'a str,
    show_name: &'a str,
    skip_confirm: bool,
    matcher: &'a dyn Matcher,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Rename(args) => rename(args),
        Command::Split(args) => split(args),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn split(args: SplitArgs) -> Result<()> {
    let mut failed = false;
    for input_path in &args.inputs {
        println!("Splitting: {input_path:?}");
        match splitter::split_by_chapter(input_path) {
            Ok(outputs) => println!("Wrote {} file(s)\n", outputs.len()),
            Err(e) => {
                eprintln!("Error splitting {input_path:?}: {e:#}\n");
                failed = true;
            }
        }
    }

    if failed {
        bail!("Some files could not be split");
    }
    Ok(())
}

fn rename(cli: RenameArgs) -> Result<()> {
    // A prefixed --show-id (tmdb-926) names the provider itself
    let (id_provider, bare_id) = match cli.show_id.as_deref().map(parse_show_id) {
        Some((provider, id)) => (provider, Some(id)),
        None => (None, None),
    };
    if let (Some(flag), Some(prefix)) = (cli.provider, id_provider) {
        if flag != prefix {
            bail!(
                "--provider {} conflicts with --show-id {}",
                flag.prefix(),
                cli.show_id.as_deref().unwrap_or_default()
            );
        }
    }
    if cli.show.is_some() && bare_id.is_some() {
        bail!("Error: Cannot specify both --show and --show-id");
    }
    if cli.show.is_none() && bare_id.is_none() {
        bail!("Error: Must specify either --show or --show-id");
    }

    let (provider, api_key) = config::choose_provider(
        cli.provider.or(id_provider),
        config::get_tmdb_api_key()?,
        config::get_tvdb_api_key()?,
    )?;
    println!("Using {}", provider.label());
    let mut client: Box<dyn MetadataProvider> = match provider {
        Provider::Tmdb => Box::new(TmdbClient::new(api_key)),
        Provider::Tvdb => Box::new(TvdbClient::new(api_key)),
    };

    // Jev is optional and only used for subtitle matching
    let jev = match config::get_jev_config()? {
        Some(config) if cli.match_mode == MatchMode::Subtitles => {
            println!("Using Jev model {}", config.model);
            Some(JevClient::new(config)?)
        }
        _ => None,
    };

    // Load cache
    let mut cache = Cache::load();

    // Determine show ID
    let provider_id = match (cli.show, bare_id) {
        (Some(show_name), None) => match search_and_select_show(client.as_mut(), &show_name) {
            Ok(id) => id,
            Err(e) => {
                bail!("Error searching for show: {e}");
            }
        },
        (None, Some(id)) => id,
        _ => unreachable!("--show / --show-id were checked above"),
    };
    // What the cache and matchers know the series by
    let show_id = cache_key(provider, &provider_id);

    // Preload cache with series name and all episodes (only if not already cached). Caches from
    // before overviews and runtimes were stored are refreshed when Jev needs them.
    let mut needs_preload = !cache.has_series_episodes(&show_id);
    if !needs_preload && jev.is_some() && !cache.is_series_detailed(&show_id) {
        println!("Refreshing cache to fetch episode overviews and runtimes...");
        needs_preload = true;
    }
    if needs_preload {
        println!("Preloading episode cache for series {show_id}...");
        client.preload_episodes(&provider_id, &show_id, &mut cache)?;
        println!("Cache preloaded successfully.");
        // Persist right away: the preload is slow, and bailing or being interrupted later in the
        // run would otherwise throw it away and force a refetch next time.
        if let Err(e) = cache.save() {
            eprintln!("Warning: Failed to save cache: {e}");
        }
    } else {
        println!("Using cached episode data for series {show_id}");
    }

    // Get show name from cache or API
    let show_name = match get_show_name(client.as_mut(), &provider_id, &show_id, &mut cache) {
        Ok(name) => name,
        Err(e) => {
            bail!("Error getting show name: {e}");
        }
    };

    if let Some(season) = cli.season {
        if jev.is_none() && cli.match_mode != MatchMode::TitleCard {
            eprintln!(
                "Warning: --season only affects Jev and title-card matching and will be ignored."
            );
        } else if !cache
            .episodes_for_series(&show_id)
            .iter()
            .any(|ep| ep.season_number == season)
        {
            bail!("--season {season} is not a season of this series");
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
            accept_unchanged: cli.accept_unchanged,
            season: cli.season,
            include_specials: cli.include_specials,
            jev,
        }),
        MatchMode::TitleCard => Box::new(TitleCardMatcher {
            start_seconds: cli.title_start,
            duration_seconds: cli.title_duration,
            skip_confirm: cli.no_confirm,
            season: cli.season,
            include_specials: cli.include_specials,
        }),
    };

    let session = Session {
        series_id: &show_id,
        show_name: &show_name,
        skip_confirm: cli.no_confirm,
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
        let ctx = MatchContext {
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

fn search_and_select_show(client: &mut dyn MetadataProvider, query: &str) -> Result<String> {
    let results = client.search_series(query)?;

    if results.is_empty() {
        bail!("No shows found matching '{query}'");
    }

    if results.len() == 1 {
        return Ok(results[0].id.clone());
    }

    // Multiple results - let user select
    println!("Multiple shows found. Please select one:");
    for (i, result) in results.iter().enumerate() {
        let year = result
            .year
            .as_ref()
            .map(|y| format!(" ({y})"))
            .unwrap_or_default();
        println!("  {}: {}{year} (ID: {})", i + 1, result.name, result.id);
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
            return Ok(results[choice - 1].id.clone());
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
) -> Result<()> {
    if file_path.extension().and_then(|s| s.to_str()) != Some("mkv") {
        bail!("Skipping non-MKV file: {file_path:?}");
    }

    println!("Processing: {file_path:?}");

    let matched = session
        .matcher
        .match_episode(file_path, session.series_id, cache, ctx)?;

    let Some(EpisodeMatch { episode, confirmed }) = matched else {
        eprintln!("Warning: No matching episode found for {file_path:?}");
        return Ok(());
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
    renamer::rename_file(file_path, &new_path, session.skip_confirm || confirmed)?;
    Ok(())
}

fn process_directory(
    dir_path: &Path,
    session: &Session,
    recursive: bool,
    cache: &mut Cache,
) -> Result<()> {
    let mkv_files = collect_mkv_files(dir_path, recursive)?;

    println!("Found {} MKV file(s) to process", mkv_files.len());

    let ctx = MatchContext {
        show_name: session.show_name,
    };
    for file_path in &mkv_files {
        if let Err(e) = process_file(file_path, session, cache, &ctx) {
            eprintln!("Error processing {file_path:?}: {e}");
            // Continue processing other files
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

fn get_show_name(
    client: &mut dyn MetadataProvider,
    provider_id: &str,
    show_id: &str,
    cache: &mut Cache,
) -> Result<String> {
    if let Some(name) = cache.get_series_name(show_id) {
        return Ok(name.clone());
    }
    let name = client.series_name(provider_id)?;
    cache.set_series_name(show_id.to_string(), name.clone());
    Ok(name)
}
