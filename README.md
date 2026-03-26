# fastgrep

**Agent-friendly fast regex search tool** — powered by a trigram inverted index, achieving 10–70× speedup on large codebases.

## Motivation

In Agent workflows, `grep`/`rg` is one of the most frequent operations. When a codebase grows to tens of thousands of files, ripgrep's full-text scan can take hundreds of milliseconds or even exceed 15 seconds. `fastgrep` builds a trigram inverted index to narrow candidates down to a handful of files first, then runs the full regex match only on those files, drastically reducing search latency.

```
# Searching for "HashMap" in a repo with 2,244 files
$ fastgrep search "HashMap"
[fastgrep] 16 matches in 6 candidates / 2244 total files (index: used)
#                          ↑ only 6 files scanned, not all 2,244
```

## Quick Start

### Installation

```bash
# Build and install from source
cd /path/to/fastgrep
bash scripts/install.sh

# Or build manually
cargo build --release -p fastgrep-cli
cp target/release/fastgrep ~/.local/bin/
```

Make sure `~/.local/bin` is in your `$PATH`.

### Up and Running in 30 Seconds

```bash
# Enter your project directory
cd /path/to/your/project

# Build the index (first time only — subsequent searches maintain it automatically)
fastgrep index

# Search!
fastgrep search "HashMap"
```

## Command Reference

### `fastgrep index` — Build the Index

Scans all files under a directory, extracts trigrams, and writes the index to the `.fastgrep/` directory. Large files (>1 MB) are automatically skipped. Progress is displayed in real time during indexing.

```bash
fastgrep index                    # Index the current directory
fastgrep index --path /some/repo  # Index a specific directory
```

**Example output:**
```
Building index for /data/home/user/linux...
Index built: 74521 files, 389204 trigrams in 2341ms
```

**Notes:**
- Automatically respects `.gitignore`; skips `.git/` and `.fastgrep/` directories
- Automatically detects and skips binary files (checks the first 8 KB for null bytes)
- Automatically skips files larger than 1 MB
- Uses Rayon for multi-core parallel processing

### `fastgrep search` — Search

```bash
fastgrep search <PATTERN> [OPTIONS]
```

#### Basic Search

```bash
# Literal search
fastgrep search "HashMap"

# Regex search
fastgrep search "impl\s+\w+\s+for\s+\w+"

# Alternation
fastgrep search "(TODO|FIXME|HACK)"
```

#### Options

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--before-context <N>` | `-B` | Number of context lines before a match | 0 |
| `--after-context <N>` | `-A` | Number of context lines after a match | 0 |
| `--context <N>` | `-C` | Context lines before and after (overrides -A/-B) | — |
| `--ignore-case` | `-i` | Case-insensitive search (uses lowercase-folded trigrams from index) | false |
| `--type <EXT>` | `-t` | Filter by file extension | — |
| `--glob <PATTERN>` | `-g` | Filter files by glob pattern | — |
| `--format <FMT>` | `-f` | Output format: `text` or `json` | text |
| `--path <DIR>` | `-p` | Directory to search | current dir |
| `--no-auto-index` | — | Disable automatic index build/rebuild | false |

#### Practical Examples

```bash
# Search for TODO/FIXME with context
fastgrep search "(TODO|FIXME)" -C 2

# Search only in Rust files
fastgrep search "pub fn" -t rs

# Search only in TSX files
fastgrep search "useState" -g "*.tsx"

# Case-insensitive search
fastgrep search "error" -i

# JSON output (ideal for agent/script consumption)
fastgrep search "HashMap" --format json

# Specify a directory
fastgrep search "HashMap" --path /path/to/repo
```

#### Output Formats

**Text format** (default, ripgrep-compatible):
```
crates/fastgrep-core/src/index/builder.rs
42:use crate::ngram::extract::extract_trigrams;
```

File names are highlighted in magenta, line numbers in green. Respects the `NO_COLOR` environment variable.

**JSON format** (`--format json`, one JSON object per line):
```json
{"file":"src/index/builder.rs","line_number":42,"line":"use crate::ngram::extract::extract_trigrams;"}
```

**Statistics** (always printed to stderr):
```
[fastgrep] 16 matches in 6 candidates / 2244 total files (index: used)
```

### `fastgrep status` — View Index Status

```bash
fastgrep status
```

**Example output:**
```
Index status:
  Root:         /data/home/user/fastgrep
  Files:        2244
  Trigrams:     14827
  Commit:       a1b2c3d4e5f6...
  Fresh:        yes
  Index size:   416 KB (lookup: 231 KB, postings: 184 KB)
```

If the index is stale (HEAD commit differs from the one recorded at build time), it will show:
```
  Fresh:        NO — rebuild recommended
```

## Automatic Index Management

Default behavior (auto-index enabled; disable with `--no-auto-index`):

1. **First search**: no index exists → build automatically
2. **Subsequent searches**: index exists → check freshness
3. **Stale index**: HEAD has changed → rebuild automatically
4. **Fresh index**: use as-is, zero overhead

**Freshness model:**
- **Git repositories**: freshness is determined by comparing the current HEAD commit hash against the one stored in the index. When the index is fresh but there are uncommitted changes, a delta overlay layer is applied to cover those changes.
- **Non-git directories**: the existing index is trusted as-is. Rebuild manually with `fastgrep index` when the directory contents change.

```bash
# Don't want automatic indexing? Manage it manually:
fastgrep search "pattern" --no-auto-index
```

## When to Use fastgrep vs rg

| Scenario | Recommended Tool |
|----------|------------------|
| Large repo (>10k files), repeated searches | **fastgrep** ✅ |
| Patterns containing rare literals | **fastgrep** ✅ |
| High-frequency searches in Agent workflows | **fastgrep** ✅ |
| Small repo (<1k files) | rg (index overhead not worthwhile) |
| Pure regex with no literals (`.*`, `\d+`) | rg (cannot leverage index) |
| One-off searches | rg (no need to build an index) |

**Core principle**: the more literals in the pattern — and the rarer they are — the greater fastgrep's advantage.

## Using as a Claude Code Skill

### Install the Skill

```bash
cp skill/fastgrep.md ~/.claude/skills/
```

Once installed, Claude Code can prefer `fastgrep search` over `rg` when searching large codebases.

## Benchmark: fastgrep vs ripgrep

### Quick Start

```bash
# 1. Prepare a corpus (pick one)

# Option A: Generate a synthetic corpus of 10,000 files (recommended for first try)
fastgrep-bench prepare --corpus medium --output ./testdata
CORPUS=./testdata/medium

# Option B: Use your own project directory
CORPUS=/path/to/your/project

# Option C: Clone the Linux Kernel for extreme benchmarking (~74,000 files)
git clone --depth 1 https://github.com/torvalds/linux.git ./testdata/linux-kernel
CORPUS=./testdata/linux-kernel

# 2. Run the benchmark (median of 10 iterations)
fastgrep-bench run --corpus $CORPUS --iterations 10 --output results.csv

# 3. Generate the report
fastgrep-bench report --input results.csv
```

> **rg not in PATH?** Specify it via environment variable: `RG_PATH=/path/to/rg fastgrep-bench run ...`

### Test Patterns

The benchmark covers 9 patterns representing typical Agent search scenarios:

| Pattern | Type | Description |
|---------|------|-------------|
| `fn` | Literal (common) | Present in almost every file; index has no advantage |
| `HashMap` | Literal (medium) | Present in some files; index filters out most |
| `SPDX-License-Identifier` | Literal (rare) | Present in very few files; **greatest speedup** |
| `pub fn new` | Literal (multi-word) | Multiple trigram intersections; excellent filtering |
| `fn\s+\w+\s*\(` | Regex | Contains literal `fn`; partially optimizable |
| `use\s+\w+::\w+` | Regex | Contains literal `use`; partially optimizable |
| `impl\s+\w+\s+for\s+\w+` | Regex | Contains `impl` + `for`; multiple literal segments |
| `(TODO\|FIXME\|HACK)\b` | Regex (alternation) | Three alternatives; index takes their union |
| `.*` | Non-optimizable | No literals; falls back to full scan (control group) |

### Real-World Benchmark Results (1,909 files)

```
| Pattern          | rg (ms) | fastgrep (ms) | Speedup |
|------------------|---------|---------------|---------|
| literal_rare     |  175.8  |    49.8       |  3.5x   |
| literal_medium   |  173.9  |    47.1       |  3.7x   |
| literal_pub_fn   |  181.3  |    47.7       |  3.8x   |
| regex_impl_trait |  190.0  |    79.2       |  2.3x   |
| regex_use_stmt   |  180.8  |   114.6       |  1.5x   |
| regex_todo       |  251.7  |   180.3       |  1.4x   |
| literal_common   |  167.2  |   173.9       |  1.0x   |
| regex_fn_decl    |  176.3  |   177.3       |  1.0x   |
| regex_dot_star   | 4296.2  |  1162.9       |  3.7x   |
```

**Takeaway**: the rarer the pattern, the greater the speedup; the larger the repo (more files), the higher the gains.

> For the full benchmark methodology, see [BENCHMARK.md](BENCHMARK.md).

## Project Structure

```
fastgrep/
├── Cargo.toml                        # Workspace root
├── crates/
│   ├── fastgrep-core/src/            # Core library
│   │   ├── ngram/extract.rs          #   Trigram extraction + FNV-1a hashing
│   │   ├── ngram/weight.rs           #   CRC32 weighting + character-pair frequency table
│   │   ├── index/format.rs           #   On-disk format definition
│   │   ├── index/posting.rs          #   Varint encoding + set operations
│   │   ├── index/builder.rs          #   Parallel index building
│   │   ├── index/writer.rs           #   Index serialization
│   │   ├── index/reader.rs           #   Mmap reader + binary search
│   │   ├── index/delta.rs            #   Uncommitted-changes overlay layer
│   │   ├── query/decompose.rs        #   Regex → trigram decomposition
│   │   ├── query/plan.rs             #   Query plan optimization
│   │   ├── query/execute.rs          #   Search execution engine
│   │   └── git.rs                    #   Git integration
│   ├── fastgrep-cli/src/             # CLI
│   │   ├── main.rs                   #   clap entry point
│   │   ├── cmd/{index,search,status}.rs
│   │   └── output.rs                 #   Output formatting
│   └── fastgrep-bench/src/           # Benchmark tool
├── skill/fastgrep.md                 # Claude Code Skill definition
└── scripts/install.sh                # Install script
```

## Dependencies

| Purpose | Crate |
|---------|-------|
| Regex engine | `regex` + `regex-syntax` |
| Memory mapping | `memmap2` |
| Hashing | `crc32fast`, FNV-1a (built-in) |
| Byte order | `byteorder` |
| CLI | `clap` (derive mode) |
| File traversal | `ignore` (.gitignore-aware) |
| Glob matching | `globset` |
| Parallelism | `rayon` |
| Error handling | `anyhow` + `thiserror` |
| Git | `gix` |
| Serialization | `serde` + `serde_json` |

## License

MIT
