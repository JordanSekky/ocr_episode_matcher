# Episode Matcher

**⚠️ Work in Progress (WIP) Software**

A Rust CLI tool that extracts production codes from MKV video files using OCR and automatically renames them using metadata from TheTVDB API.

## ⚠️ Important Note

This software is currently **work in progress** and has only been tested and confirmed to work with files from **"The X-Files" blu-ray set**. While it may work with other shows, it has not been extensively tested and may require adjustments for different production code formats or video file structures.

## Features

- **OCR-based Production Code Extraction**: Extracts production codes (e.g., `#3X22`, `#6ABX08`) from video frames using optical character recognition
- **TVDB Integration**: Automatically looks up episode information using TheTVDB API v4
- **Smart Renaming**: Renames files to format: `{Show Name} - S{season}E{episode} - {Episode Title}.mkv`
- **Batch Processing**: Process entire directories of MKV files
- **Caching**: Caches TVDB data locally to avoid repeated API calls
- **Interactive Mode**: Prompts for confirmation before renaming (can be skipped with `--no-confirm`)
- **Subtitle Mode**: View embedded subtitles (SRT/PGS) to manually identify episodes, optionally with episode suggestions from a local Ollama model

## Requirements

- **Rust** (latest stable version)
- **FFmpeg** (must be installed and available in PATH)
- **TheTVDB API Key** (get one at [thetvdb.com](https://thetvdb.com))

## Installation

1. Clone this repository:
   ```bash
   git clone <repository-url>
   cd episode-matcher
   ```

2. Build the release version:
   ```bash
   cargo build --release
   ```

3. The binary will be at `target/release/episode-matcher`

## Configuration

### TVDB API Key

You need to provide your TheTVDB API key in one of two ways:

**Option 1: Environment Variable**
```bash
export TVDB_API_KEY="your-api-key-here"
```

**Option 2: Config File**
Create `~/.episode-matcher/config.toml`:
```toml
tvdb_api_key = "your-api-key-here"
```

### Ollama (optional, subtitle mode only)

Subtitle mode can ask an [Ollama](https://ollama.com) model to match each file's subtitles against TheTVDB episode descriptions. It is enabled by environment variables:

```bash
export EPISODE_MATCHER_OLLAMA_URL="http://localhost:11434"   # enables the feature
export EPISODE_MATCHER_OLLAMA_MODEL="your-model"              # required when the URL is set
export EPISODE_MATCHER_OLLAMA_NUM_CTX="32768"                 # optional, defaults to 32768
```

Without `EPISODE_MATCHER_OLLAMA_URL`, subtitle mode behaves exactly as before.

## Usage

### Basic Usage

Process a single file:
```bash
episode-matcher "/path/to/file.mkv" --show-id 77398
```

Process a directory:
```bash
episode-matcher "/path/to/directory" --show-id 77398
```

Process multiple files or directories:
```bash
episode-matcher "/path/to/dir1" "/path/to/dir2" "/path/to/file.mkv" --show-id 77398
```

### Command Line Options

- `<inputs>...` - Input files or directories to process (required, one or more)
- `-r, --recursive` - Recursively scan directories for MKV files
- `--show <name>` - Show name to search in TheTVDB (will prompt for selection if multiple matches)
- `--show-id <id>` - Direct TheTVDB show ID (faster, no search needed)
- `--match-mode <mode>` - Matching strategy: `prod-code` (default) or `subtitles`
- `--no-confirm` - Skip confirmation prompts (useful for batch processing)
- `--prompt-size <bytes>` - For files larger than this size, prompt for production code manually if OCR fails (only for `prod-code` mode)
- `--subtitle-duration <seconds>` - How much of each file's subtitles to extract (only for `subtitles` mode, default 300)
- `--hint <SxxExx>` - Roughly where the first file of each input directory sits in the series, e.g. `S03E01` (only used with Ollama; see below)

### Examples

**Using show name:**
```bash
episode-matcher "/path/to/videos" --show "The X-Files"
```

**Using show ID (faster):**
```bash
episode-matcher "/path/to/videos" --show-id 77398 --no-confirm
```

**Using Subtitle Matching Mode:**
```bash
episode-matcher "/path/to/videos" --show-id 77398 --match-mode subtitles
```
This will extract English subtitles (SRT or PGS) and display them to you. You then manually enter the Season/Episode (e.g., S01E01). For PGS subtitles, it uses OCR to convert the image-based subs to text.

**Using Subtitle Matching Mode with Ollama:**
```bash
EPISODE_MATCHER_OLLAMA_URL=http://localhost:11434 EPISODE_MATCHER_OLLAMA_MODEL=your-model \
  episode-matcher "/path/to/disc" --show-id 77398 --match-mode subtitles
```
For each file, the model is shown the subtitle text, the episode descriptions and runtimes, and the length of the file in minutes (a weak hint only: TVDB runtimes are often broadcast slots including ads, so files are usually a few minutes shorter; roughly double the length or more suggests a combined file), and suggests an episode with its reasoning. The current file's name is withheld from the prompt, since it may carry an earlier run's (possibly wrong) answer. At the prompt:

- press Enter to accept the suggestion
- type an episode (e.g. `S01E05`) to override it
- type `v` to view the subtitles in your pager first

Duplicates are fine: rips often contain several copies of the same episode, and some files are long ones with several or all of the disc's episodes back to back. The model is told this, and for a combined file it answers with the first episode, since only the start of the subtitles is read.

Not every file is an episode (extras, recaps and play-all tracks are common on discs), so the model may answer "no match". In that case Enter skips the file, and you can still type an episode to assign one by hand. With `--no-confirm`, suggestions are accepted automatically and no-match files are skipped.

The model must also give the episode's title, and the answer is rejected if the title doesn't belong to the season and episode it names, so a reply that contradicts itself is never accepted. A rejected answer prints the reason and, with `--no-confirm`, skips the file; otherwise it falls back to the manual flow above, as does an unreachable Ollama.

**Rip locality:** files on a disc are usually in episode order, so files that sort next to each other tend to be neighbouring episodes. Within a single run, the prompt includes the neighbouring filenames and what they were matched to, and once an earlier file has been matched, only episodes within about 7 of where the current file is expected to be are offered to the model. Process a whole disc directory in one run to benefit from this. Files you skip, or decline to rename, don't count as hints.

**Big series:** until the first file is matched there is nothing to narrow the list, so the whole series is sent. Pass `--hint` to start narrowed:
```bash
episode-matcher "/path/to/disc" --show-id 77398 --match-mode subtitles --hint S03E01
```
The first file in each input directory is expected near S03E01, the next one episode later, and so on, with the same ±7 window. Once a file has actually been matched, that match takes over from the hint. Because the hint applies to every input directory, run discs from different parts of the series separately.

The first Ollama run for a show re-downloads its episode data once, to fetch episode descriptions and runtimes. Long series produce long prompts, so use a model with a large context window and raise `EPISODE_MATCHER_OLLAMA_NUM_CTX` if needed. Ollama silently drops the start of a prompt that doesn't fit, so the tool warns when a prompt filled the whole window.

**Process multiple directories:**
```bash
episode-matcher "/path/to/season1" "/path/to/season2" "/path/to/season3" --show-id 77398 --no-confirm
```

**Recursively scan a directory:**
```bash
episode-matcher -r "/path/to/videos" --show-id 77398
```

**Prompt for production code on large files:**
```bash
episode-matcher "/path/to/videos" --show-id 77398 --prompt-size 1073741824
```
This will prompt you to manually enter the production code for files larger than 1GB (1073741824 bytes), if the OCR does not find a valid production code. 

## How It Works

1. **Frame Extraction**: Extracts frames from the last 15 seconds of the video at 1 fps
2. **OCR Processing**: Uses OCR to find production codes in the extracted frames
3. **TVDB Lookup**: Queries TheTVDB API using the production code to get episode metadata
4. **File Renaming**: Renames the file using the format: `{Show Name} - S{season}E{episode} - {Episode Title}.mkv`

## Production Code Formats Supported

The tool recognizes several production code formats:

- **The X Files Seasons 1-5**: `#3X22`, `#1X79`
- **The X Files Seasons 6-9**: `#6ABX08`, `#7ABX14`
- **The X Files Seasons 10-11**: `#1AYW01`, `#2AYW01`

The regex pattern is case-insensitive and handles spaces around the X.

## Caching

The tool caches TVDB data locally at `~/.episode-matcher/cache.json` to:
- Speed up subsequent runs
- Reduce API calls
- Work offline for previously cached shows

The cache stores:
- Series names (mapped by series ID)
- Episode information (mapped by production code and by season/episode), including descriptions, air dates and runtimes

## Limitations

- **WIP Status**: This software is work in progress and has only been tested with "The X-Files" blu-ray set
- **Production Code Formats**: May not recognize all production code formats used by other shows
- **OCR Accuracy**: Depends on video quality and production code visibility
- **File Format**: Currently only supports MKV files
- **Video Stream Requirement**: Files must contain a video stream (audio-only files will be skipped)

## Troubleshooting

### FFmpeg not found
Make sure FFmpeg is installed and available in your PATH:
```bash
ffmpeg -version
```

### Tesseract not found or tessdata missing
The tool requires Tesseract OCR to be installed with English language data. Install it using:

**macOS:**
```bash
brew install tesseract tesseract-lang
```

**Ubuntu/Debian:**
```bash
sudo apt-get install tesseract-ocr tesseract-ocr-eng
```

**Other systems:**
Install tesseract-ocr and the English language data package for your distribution.

You can also set the `TESSDATA_PREFIX` environment variable to point to your tessdata directory:
```bash
export TESSDATA_PREFIX="/path/to/tessdata"
```

### Production code not found
- Ensure the video file has a production code visible in the last 15 seconds
- Check that the video has a video stream (not audio-only)
- Try processing the file again (OCR can be inconsistent)

## Contributing

This is hobby software, feel free to fork and make a PR, but I can't make any support guarantees.

## License

MIT License

## Acknowledgments

- Uses [tesseract-rs](https://crates.io/crates/tesseract-rs) for OCR functionality (wraps [Tesseract OCR](https://github.com/tesseract-ocr/tesseract))
- Integrates with [TheTVDB API v4](https://thetvdb.com/api-information)
