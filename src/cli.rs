use clap::{Parser, ValueEnum};
use std::path::PathBuf;

use crate::media::subtitles::SubtitleCodec;

#[derive(Debug, Clone, ValueEnum, PartialEq, Eq)]
pub enum MatchMode {
    ProductionCode,
    Subtitles,
}

#[derive(Parser)]
#[command(name = "episode-matcher")]
#[command(about = "Extract production codes from video files and rename them using TVDB data")]
pub struct Cli {
    /// Input files or directories to process
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Show name to search in TVDB
    #[arg(long)]
    pub show: Option<String>,

    /// Direct TVDB show ID
    #[arg(long)]
    pub show_id: Option<String>,

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

    /// Also offer specials (season 0) to Jev. They are left out by default because they are
    /// mostly extras and short webisodes; they can still be entered by hand.
    #[arg(long)]
    pub include_specials: bool,

    /// Only offer Jev episodes from this season (e.g. 3). Without it the whole series is
    /// offered.
    #[arg(long, value_name = "N")]
    pub season: Option<u64>,
}
