---
name: fastgrep
description: Fast regex search with trigram indexing — 10-70x faster than ripgrep on large codebases
---

# fastgrep — Fast Regex Search

`fastgrep` is a trigram-indexed search tool optimized for large codebases. It builds an inverted index of character trigrams, then uses the index to narrow candidate files before running full regex matching.

## When to Use

- **Large repositories** (>10k files): fastgrep shines when the index can eliminate most files
- **Repeated searches**: The index is built once, subsequent searches are near-instant
- **High-selectivity patterns**: Patterns with rare literal substrings benefit most (e.g., specific function names, identifiers)

## When NOT to Use

- **Small repositories** (<1k files): `rg` is already fast enough, index overhead isn't worth it
- **Patterns with no literals** (e.g., `.*`, `\d+`): Cannot be optimized by trigram index, falls back to full scan
- **One-off searches**: If you'll only search once, building the index adds overhead

## Usage

### Build Index
```bash
fastgrep index [--path <dir>]
```

### Search
```bash
# Basic search (auto-builds index if missing)
fastgrep search "HashMap"

# Regex search
fastgrep search "impl\s+\w+\s+for\s+\w+"

# With context lines
fastgrep search -C 3 "TODO|FIXME"

# Case insensitive
fastgrep search -i "error"

# Filter by file type
fastgrep search -t rs "pub fn"

# JSON output (for programmatic parsing)
fastgrep search --format json "pattern"

# Glob filter
fastgrep search -g "*.tsx" "useState"
```

### Check Index Status
```bash
fastgrep status
```

## Output Format

### Text (default)
ripgrep-compatible format:
```
path/to/file.rs
42:    pub fn new() -> Self {
```

### JSON (`--format json`)
JSON Lines format, one match per line:
```json
{"file":"path/to/file.rs","line_number":42,"line":"    pub fn new() -> Self {"}
```

## How It Works

1. **Index**: Extracts character trigrams from every file, builds an inverted index mapping trigram → file IDs
2. **Query**: Decomposes the regex pattern into required trigrams
3. **Filter**: Looks up trigrams in the index, intersects posting lists to find candidate files
4. **Verify**: Runs full regex only on candidate files

Example: Searching for `impl.*Display` in a 74k-file codebase:
- Extracts trigrams: `imp`, `mpl`, `Dis`, `isp`, `spl`, `pla`, `lay`
- Index lookup finds only 12 files contain all these trigrams
- Full regex runs on 12 files instead of 74,000 → **~60x speedup**
