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
- **Subtitle Mode**: View embedded subtitles (SRT/PGS) to manually identify episodes

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
- Episode information (mapped by production code)

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
