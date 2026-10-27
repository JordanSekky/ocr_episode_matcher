use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::infra::metadata::Provider;
use crate::media::subtitles::SubtitleCodec;

#[derive(Debug, Clone, ValueEnum, PartialEq, Eq)]
pub enum MatchMode {
    ProductionCode,
    Subtitles,
    /// Read each short's title card and pick the episode whose title is closest to it
    TitleCard,
}

#[derive(Parser)]
#[command(name = "episode-matcher")]
#[command(
    about = "Rename ripped episodes using TMDB (or TVDB) data, and split multi-title rips by chapter"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Identify video files and rename them using TMDB (or TVDB) data
    Rename(RenameArgs),
    /// Split files into one file per chapter, next to the source file
    Split(SplitArgs),
}

#[derive(Args)]
pub struct SplitArgs {
    /// Files to split
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
}

#[derive(Args)]
pub struct RenameArgs {
    /// Input files or directories to process
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Show name to search for
    #[arg(long)]
    pub show: Option<String>,

    /// Show ID on the metadata provider. A `tmdb-` or `tvdb-` prefix, as in Plex and Jellyfin
    /// folder names (`tmdb-926`), picks the provider
    #[arg(long)]
    pub show_id: Option<String>,

    /// Where to get episode data: TMDB by default, or TVDB. Falls back to TVDB when only a TVDB
    /// key is set
    #[arg(long, value_enum)]
    pub provider: Option<Provider>,

    /// Skip confirmation prompts
    #[arg(long)]
    pub no_confirm: bool,

    /// Don't ask to confirm a Jev suggestion when the file already has that episode's name;
    /// move straight on to the next file
    #[arg(long)]
    pub accept_unchanged: bool,

    /// Recursively scan directories for MKV files
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,
    /// File size where the user is prompted for the production code

    #[arg(long = "prompt-size")]
    pub prompt_size: Option<u64>,

    /// Matching mode
    #[arg(long, default_value = "production-code")]
    pub match_mode: MatchMode,

    /// Seconds into each file to start reading subtitles, e.g. to skip a "previously on" recap
    #[arg(long = "subtitle-start", default_value_t = 0)]
    pub subtitle_start: u64,

    /// Maximum duration (in seconds) to extract subtitles for
    #[arg(long = "subtitle-duration", default_value_t = 300)]
    pub subtitle_duration: u64,

    /// Subtitle codec to use when a file has several English tracks (falls back to the others
    /// if it has none of this codec)
    #[arg(long = "prefer-codec", value_enum, default_value = "subrip")]
    pub prefer_codec: SubtitleCodec,

    /// Seconds into each file to start reading the title card (title-card mode). The default
    /// skips the studio logo that opens each short.
    #[arg(long = "title-start", default_value_t = 8)]
    pub title_start: u64,

    /// How many seconds of the file to read for the title card (title-card mode)
    #[arg(long = "title-duration", default_value_t = 12)]
    pub title_duration: u64,

    /// Also offer specials (season 0) to Jev, or to match against in title-card mode. They are
    /// left out by default because they are mostly extras and short webisodes; they can still
    /// be entered by hand.
    #[arg(long)]
    pub include_specials: bool,

    /// Only offer episodes from this season (e.g. 3) to Jev, or to match against in title-card
    /// mode. Without it the whole series is offered.
    #[arg(long, value_name = "N")]
    pub season: Option<u64>,
}
