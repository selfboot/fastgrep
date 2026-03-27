# fastgrep Technical Implementation Report

## 1. System Architecture Overview

fastgrep is a fast regex search tool based on trigram inverted indexes. The core idea comes from the field of information retrieval: **first use an inverted index to quickly narrow down the candidate set, then perform exact matching on the candidate files**.

### 1.1 Architecture Diagram

```
┌─────────────────────────────────────────────────────┐
│                    CLI Layer                         │
│  fastgrep index | fastgrep search | fastgrep status  │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│                  Query Pipeline                      │
│                                                      │
│  ┌──────────┐   ┌──────────┐   ┌──────────────────┐ │
│  │ Decompose│──▶│   Plan   │──▶│     Execute      │ │
│  │ (regex →  │   │ (sort by │   │ (lookup →        │ │
│  │  trigrams)│   │  select.)│   │  intersect →     │ │
│  └──────────┘   └──────────┘   │  verify regex)   │ │
│                                 └──────────────────┘ │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│                  Index Layer                         │
│                                                      │
│  ┌───────────────┐  ┌─────────────┐  ┌───────────┐  │
│  │ index.lookup  │  │ index.post- │  │ index.meta│  │
│  │ (mmap, binary │  │ ings (varint│  │ (JSON,    │  │
│  │  search)      │  │  delta enc) │  │  file map)│  │
│  └───────────────┘  └─────────────┘  └───────────┘  │
└─────────────────────────────────────────────────────┘
```

### 1.2 Complete Search Flow Example

Using the search for `impl.*Display` as an example:

```
Input: "impl.*Display"
                │
        ┌───────▼───────┐
  Step 1│  regex-syntax  │  Parse into HIR (High-level IR)
        │  AST traversal │
        └───────┬───────┘
                │  Extract literal substrings: ["impl", "Display"]
        ┌───────▼───────┐
  Step 2│  Trigram decomp│  "impl" → [imp, mpl]
        │                │  "Display" → [Dis, isp, spl, pla, lay]
        └───────┬───────┘
                │  must_match = [hash(imp), hash(mpl), hash(Dis), ...]
        ┌───────▼───────┐
  Step 3│  Query plan    │  Sort by posting list size
        │                │  Rarest trigrams first
        └───────┬───────┘
                │  ordered = [hash(Dis), hash(isp), hash(mpl), ...]
        ┌───────▼───────┐
  Step 4│  Index lookup  │  Binary search the lookup table
        │  + intersection│  Intersect posting lists one by one
        │                │  Early termination: return immediately if intersection is empty
        └───────┬───────┘
                │  candidate_file_ids = [12, 45, 203]
        ┌───────▼───────┐
  Step 5│  Full-text     │  Run full regex on only 3 files
        │  verification  │  (instead of scanning all 74k files)
        └───────┬───────┘
                │
             Output results
```

---

## 2. On-Disk Index Format

The index is stored in the `.fastgrep/` directory and consists of three files.

### 2.1 index.lookup — Lookup Table

```
Offset    Size      Field             Description
─────────────────────────────────────────────────
0x00      4B       magic             "FGLK" (0x46 0x47 0x4C 0x4B)
0x04      4B       version           1 (u32, little-endian)
─────────────────────────────────────────────────
0x08      16B      entry[0]          First lookup entry
0x18      16B      entry[1]          Second lookup entry
...
```

Each lookup entry (LookupEntry) is a fixed 16 bytes:

```
Offset    Size      Field             Type
─────────────────────────────────────────────────
+0x00     8B       ngram_hash        u64, little-endian, FNV-1a hash value
+0x08     4B       offset            u32, little-endian, offset into the postings file
+0x0C     4B       len               u32, little-endian, byte length of the posting list
```

**Key design decisions**:

- Entries are sorted in ascending order by `ngram_hash`, enabling O(log N) binary search
- Memory-mapped via mmap, no need to load the entire file
- Fixed 16-byte entry size makes random access O(1)

### 2.2 index.postings — Posting Lists

```
Offset    Size      Field             Description
─────────────────────────────────────────────────
0x00      4B       magic             "FGPS" (0x46 0x47 0x50 0x53)
0x04      4B       version           1 (u32, little-endian)
─────────────────────────────────────────────────
0x08      var      posting_list[0]   File ID list for the first trigram
...       var      posting_list[N]   File ID list for the Nth trigram
```

Encoding format for each posting list:

```
┌─────────┬────────┬────────┬────────┬─────┐
│ count   │ delta₀ │ delta₁ │ delta₂ │ ... │
│ (varint)│(varint)│(varint)│(varint)│     │
└─────────┴────────┴────────┴────────┴─────┘
```

- **count**: Number of file IDs in the list
- **delta₀**: The first file ID (i.e., the difference from 0)
- **deltaᵢ**: The difference between the i-th file ID and the (i-1)-th

**Example**: The file ID list `[5, 10, 20, 100, 1000]` is encoded as:

```
varint(5)     → count = 5
varint(5)     → delta₀ = 5       → file_id = 0 + 5 = 5
varint(5)     → delta₁ = 5       → file_id = 5 + 5 = 10
varint(10)    → delta₂ = 10      → file_id = 10 + 10 = 20
varint(80)    → delta₃ = 80      → file_id = 20 + 80 = 100
varint(900)   → delta₄ = 900     → file_id = 100 + 900 = 1000
```

### 2.3 index.meta — Metadata

JSON format, containing:

```json
{
  "version": 1,
  "file_count": 2244,
  "trigram_count": 14827,
  "commit_hash": "a1b2c3d4e5f6...",
  "files": [
    "Cargo.toml",
    "crates/fastgrep-core/src/lib.rs",
    "..."
  ]
}
```

The index position in the `files` array serves as the file ID. During search, the file ID is used to look up the path.

### 2.4 Disk Space Usage Analysis

Using a repository with 2244 files and 14827 unique trigrams as an example:

| File | Calculation | Size |
|------|------|------|
| index.lookup | 8B header + 14827 × 16B | ~231 KB |
| index.postings | 8B header + Σ posting lists | ~184 KB |
| index.meta | JSON (including file path list) | ~tens of KB |
| **Total** | | **~416 KB** |

---

## 3. Varint Encoding

Uses LEB128 (Little-Endian Base 128) variable-length integer encoding, the same format used by Protocol Buffers.

### 3.1 Encoding Rules

The most significant bit (bit 7) of each byte is the **continuation flag**:
- `1` → more bytes follow
- `0` → this is the last byte

The lower 7 bits carry the actual data, arranged from least significant to most significant.

```
Value         Encoding           Bytes
───────────────────────────────────────
0             0x00               1
1             0x01               1
127           0x7F               1
128           0x80 0x01          2
300           0xAC 0x02          2
16384         0x80 0x80 0x01     3
u32::MAX      0xFF 0xFF 0xFF 0xFF 0x0F   5
```

### 3.2 Encoding Process Example

Using 300 as an example:

```
300 in binary:  100101100
                ↓ Split into 7-bit groups
Low 7 bits:   0101100  (0x2C)
High 2 bits:  10       (0x02)

First byte:  0x2C | 0x80 = 0xAC  (continuation byte follows)
Second byte: 0x02               (last byte)

Result: [0xAC, 0x02]
```

### 3.3 Compression Effect of Delta Encoding

For ordered file ID sequences, delta encoding converts large values into small differences, which combined with varint significantly reduces storage space:

```
Original IDs: [100, 105, 110, 200, 10000]
Deltas:       [100,   5,   5,  90,  9800]

Original storage:  5 × 4B = 20B  (fixed u32)
Delta+Varint: ≈ 1 + 2 + 1 + 1 + 1 + 2 ≈ 8B  (60% compression)
```

---

## 4. Trigram Extraction and Hashing

### 4.1 FNV-1a Hash Algorithm

FNV-1a was chosen as the trigram hash function for the following reasons:
- Extremely fast computation (only XOR + multiplication)
- Uniform distribution for short inputs (3 bytes)
- Deterministic (same input always produces the same hash)

```rust
const FNV_OFFSET: u64 = 0xcbf29ce484222325;  // 64-bit offset basis
const FNV_PRIME:  u64 = 0x100000001b3;       // 64-bit prime

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u64;                    // XOR
        hash = hash.wrapping_mul(FNV_PRIME);  // Multiply by prime
    }
    hash
}
```

### 4.2 Extraction Rules

Rules for extracting trigrams from file contents:

1. **Sliding window**: Size of 3 bytes, stride of 1
2. **Skip newlines**: Any trigram containing `\n` (0x0A) is discarded
3. **Deduplication**: Each unique hash value is recorded only once per file
4. **Minimum length**: Files with fewer than 3 bytes produce no trigrams

```
Input: "Hello\nWorld"

Window traversal:
  "Hel" → hash → ✓ Keep
  "ell" → hash → ✓ Keep
  "llo" → hash → ✓ Keep
  "lo\n" → ✗ Contains newline, skip
  "o\nW" → ✗ Contains newline, skip
  "\nWo" → ✗ Contains newline, skip
  "Wor" → hash → ✓ Keep
  "orl" → hash → ✓ Keep
  "rld" → hash → ✓ Keep

Result: 6 unique trigrams
```

### 4.3 Why Skip Newline Trigrams

Trigrams containing newlines appear in nearly all files (any multi-line file contains `\n`), so their posting lists are close to the full set and have no selectivity value for search. Skipping them:
- Reduces index size
- Avoids wasted overhead during intersection operations

---

## 5. Regex Decomposition Engine

### 5.1 Overall Design

Uses the `regex-syntax` crate to parse regular expressions into HIR (High-level Intermediate Representation), then recursively traverses the HIR tree to extract literal substrings, and finally generates trigrams from those literals.

```
                Regular expression
                    │
          ┌─────────▼─────────┐
          │  regex-syntax parse│
          │  → HIR tree        │
          └─────────┬─────────┘
                    │
          ┌─────────▼─────────┐
          │  Recursive HIR     │
          │  traversal, extract│
          │  LiteralInfo       │
          └─────────┬─────────┘
                    │
          ┌─────────▼──────────┐
          │  Literals → Trigram│
          │  Generate must_match│
          │  / alternatives    │
          └────────────────────┘
```

### 5.2 HIR Node Processing Rules

| HIR Node | Processing | Output |
|-----------|---------|------|
| `Literal("abc")` | Extract directly | `Exact("abc")` |
| `Concat[a, b, c]` | Recursively extract literals from each part | `Conjunction([...])` |
| `Alternation[a \| b]` | Extract each branch; abandon if any branch has no literals | `Alternation([...])` |
| `Capture(sub)` | Recursively process sub-pattern | Same as sub-pattern |
| `Repetition{min≥1}` | Recursively process sub-pattern | Same as sub-pattern |
| `Repetition{min=0}` | Cannot be optimized | `None` |
| `Class` (character class) | Cannot be optimized | `None` |
| `Look` (assertion) | Cannot be optimized | `None` |
| `Empty` | Cannot be optimized | `None` |

### 5.3 Decomposition Examples

**Example 1: Pure literal**
```
"HashMap"
  → HIR: Literal("HashMap")
  → Exact("HashMap")
  → must_match = trigrams("HashMap")
               = [hash("Has"), hash("ash"), hash("shM"), hash("hMa"), hash("Map")]
```

**Example 2: Concatenation with wildcard**
```
r"impl\s+Display"
  → HIR: Concat[Literal("impl"), Repetition{Class(\s), min=1}, Literal("Display")]
  → Conjunction(["impl", "Display"])
  → must_match = trigrams("impl") ∪ trigrams("Display")
               = [hash("imp"), hash("mpl"), hash("Dis"), hash("isp"),
                  hash("spl"), hash("pla"), hash("lay")]
```

**Example 3: Alternation**
```
r"(TODO|FIXME|HACK)"
  → HIR: Alternation[Literal("TODO"), Literal("FIXME"), Literal("HACK")]
  → alternatives = [
      trigrams("TODO"),    // [hash("TOD"), hash("ODO")]
      trigrams("FIXME"),   // [hash("FIX"), hash("IXM"), hash("XME")]
      trigrams("HACK"),    // [hash("HAC"), hash("ACK")]
    ]
```

**Example 4: Not optimizable**
```
r".*"
  → HIR: Repetition{Class(.), min=0}
  → None
  → optimizable = false → full scan fallback
```

### 5.4 Query Semantics for Alternation

For an alternation `(A|B|C)`:

1. Each branch independently computes its trigram intersection (conjunction)
2. The results from all branches are combined via union
3. The union is then intersected with the must_match results

```
candidates = (files_matching_A ∪ files_matching_B ∪ files_matching_C) ∩ files_matching_must
```

---

## 6. Query Plan Optimization

### 6.1 Selectivity Sorting

Not all trigrams have the same filtering effectiveness. Common trigrams (e.g., `the`) may have posting lists containing many files, while rare trigrams (e.g., `Dis`) have short posting lists.

The query planner reads the byte length (`len` field) of each trigram's posting list from the index and sorts them in ascending order:

```
trigrams = [hash("Dis"), hash("imp"), hash("lay"), hash("mpl"), ...]
                 ↓              ↓            ↓           ↓
posting size:    42B           128B         256B         312B

After sorting:
ordered = [hash("Dis"), hash("imp"), hash("lay"), hash("mpl")]
```

**Advantage**: The rarest trigram is looked up first, producing the smallest intersection result → subsequent intersection operations work on minimal data.

### 6.2 Early Termination

During intersection operations, if the result becomes empty at any step, return immediately:

```rust
for &hash in &plan.ordered_trigrams {
    let posting_list = reader.lookup(hash)?;
    // ↓ If a trigram is not in the index → no match is possible
    let posting_list = match reader.lookup(hash) {
        Some(list) => list,
        None => return Vec::new(),  // Early termination
    };
    result = intersect(&current, &posting_list);
    if result.is_empty() {
        return Vec::new();  // Early termination
    }
}
```

### 6.3 Posting List Intersection Algorithm

Uses the classic **two-pointer merge-join intersection**, with time complexity O(n + m):

```rust
fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Equal   => { result.push(a[i]); i += 1; j += 1; }
            Less    => i += 1,
            Greater => j += 1,
        }
    }
    result
}
```

Prerequisite: Posting lists are sorted in ascending order by file ID (guaranteed at build time).

---

## 7. Index Build Pipeline

### 7.1 Complete Pipeline

```
┌───────────────┐    ┌──────────────────┐    ┌──────────────────┐
│ File discovery │───▶│ Parallel trigram  │───▶│ Inverted index   │
│ (ignore crate) │    │ extraction       │    │ construction     │
│                │    │ (rayon par_iter)  │    │ (BTreeMap)       │
└───────────────┘    └──────────────────┘    └────────┬─────────┘
                                                      │
                     ┌──────────────────┐    ┌────────▼─────────┐
                     │ Git HEAD detect  │───▶│ Write to disk    │
                     │ (gix crate)      │    │ (lookup+postings │
                     └──────────────────┘    │  +meta)          │
                                             └──────────────────┘
```

### 7.2 File Discovery

Uses the `ignore` crate (ripgrep's underlying traversal library), which automatically:
- Parses `.gitignore`, `.gitignore_global`, `.git/info/exclude`
- Skips hidden files
- Skips `.git/` and `.fastgrep/` directories

Returns a lexicographically sorted list of relative paths, where the array index serves as the file ID.

### 7.3 Large File Skipping

To avoid unnecessary reading and trigram extraction for large files (e.g., logs, data files, build artifacts), the builder checks file size via `std::fs::metadata()` **before** reading the file contents:

```rust
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

// Skip large files — check metadata BEFORE reading content
if let Ok(meta) = std::fs::metadata(&full_path) {
    if meta.len() > opts.max_file_size {
        skipped_large.fetch_add(1, Ordering::Relaxed);
        return None;
    }
}
```

**Key design decisions**:

- Default threshold `MAX_FILE_SIZE = 1 MB`, configurable via `BuildOptions.max_file_size`
- The `metadata()` system call only retrieves file metadata without reading content, achieving zero I/O overhead for large files
- A new `skipped_large` field in `BuildStats` reports the number of skipped large files

### 7.4 Real-Time Progress Output

During index construction, real-time progress feedback is provided via `AtomicUsize` counters:

```rust
let processed = AtomicUsize::new(0);
let total = files.len();

// Inside par_iter:
let count = processed.fetch_add(1, Ordering::Relaxed) + 1;
if count % 500 == 0 || count == total {
    eprint!("\r  Extracting trigrams... {}/{}", count, total);
}
```

- Progress is output every 500 files (using `\r` carriage return to overwrite the same line)
- Also outputs when the last file is processed, ensuring 100% completion is displayed
- Uses `AtomicUsize` + `Ordering::Relaxed` to ensure thread safety in the parallel environment while minimizing synchronization overhead

### 7.5 File ID Remapping

Not all discovered files are indexed (binary files, large files, empty files, etc. are skipped). To avoid large gaps of unused sparse IDs in posting lists, the builder performs **ID remapping** after trigram extraction:

```rust
// Collect the file IDs that were actually indexed
let mut indexed_file_ids: Vec<usize> = per_file_trigrams
    .iter().map(|(id, _)| *id).collect();
indexed_file_ids.sort_unstable();

// Build old_id → new_id mapping (consecutive numbering)
let mut id_remap: Vec<Option<u32>> = vec![None; files.len()];
let mut indexed_files: Vec<String> = Vec::with_capacity(indexed_file_ids.len());
for (new_id, &old_id) in indexed_file_ids.iter().enumerate() {
    id_remap[old_id] = Some(new_id as u32);
    indexed_files.push(files[old_id].clone());
}
```

**Effect**:

- The `files` array in `index.meta` only contains files that were actually indexed, excluding skipped binary/large files
- File IDs in posting lists are consecutive and compact (0, 1, 2, ...), improving delta encoding efficiency
- `file_count` and `indexed_count` are separated: the former is the total number of discovered files, the latter is the number of actually indexed files

### 7.6 Parallel Trigram Extraction

```rust
let per_file_trigrams: Vec<(usize, HashSet<u64>)> = files
    .par_iter()           // Rayon automatically distributes across all CPU cores
    .enumerate()
    .filter_map(|(file_id, path)| {
        let data = fs::read(full_path).ok()?;

        // Binary file detection: skip if first 8KB contains null bytes
        if data[..8192.min(data.len())].contains(&0) {
            return None;
        }

        let trigrams = extract_trigrams_with_folded(&data);
        Some((file_id, trigrams))
    })
    .collect();
```

Note: Currently uses `extract_trigrams_with_folded()` for trigram extraction, which stores both the original case and lowercase-normalized trigrams simultaneously to support index-accelerated case-insensitive search (see Section 9 for details).

### 7.7 Inverted Index Construction

```rust
let mut trigram_map: BTreeMap<u64, Vec<u32>> = BTreeMap::new();

for (file_id, trigrams) in &per_file_trigrams {
    for &hash in trigrams {
        trigram_map.entry(hash).or_default().push(file_id as u32);
    }
}

// Ensure each posting list is sorted and deduplicated
for list in trigram_map.values_mut() {
    list.sort_unstable();
    list.dedup();
}
```

Uses `BTreeMap` instead of `HashMap` to ensure the output lookup table is naturally ordered (sorted by hash value), eliminating the need for additional sorting.

### 7.8 Binary File Detection

Uses a simple but effective heuristic:

- Read the first 8192 bytes of the file
- If the data contains `0x00` (null byte), classify it as a binary file
- Binary files are excluded from the index

This method correctly identifies common binary formats such as images, compiled artifacts, and fonts, while having no false positives for text files.

---

## 8. Memory-Mapped Reading

### 8.1 Why Use mmap

| Approach | Memory Usage | Startup Time | Random Access |
|------|---------|---------|---------|
| Full load into Vec | O(file size) | High (requires read + parse) | O(1) |
| On-demand seek+read | O(1) | Low | High (system call overhead) |
| **mmap** | **O(1)¹** | **Low** | **O(1)** |

¹ The OS loads pages on demand; actual physical memory usage is far less than the file size.

### 8.2 Binary Search Implementation

```rust
pub fn lookup(&self, ngram_hash: u64) -> Option<Vec<u32>> {
    let data = &self.lookup_mmap[HEADER_SIZE..];
    let (mut lo, mut hi) = (0, self.entry_count);

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // Access mmap memory directly via offset
        let entry = self.read_lookup_entry(data, mid);

        match entry.ngram_hash.cmp(&ngram_hash) {
            Equal   => return Some(self.read_posting_list(entry.offset, entry.len)),
            Less    => lo = mid + 1,
            Greater => hi = mid,
        }
    }
    None
}
```

### 8.3 Performance Characteristics

- **Lookup complexity**: O(log N), where N = number of trigrams
- **Memory overhead**: Only the mmap descriptor; the OS manages actual pages
- **Cold start**: First access triggers page loading; subsequent accesses use the page cache
- **Concurrency safety**: Read-only mmap is inherently thread-safe

---

## 9. Case-Insensitive Search

### 9.1 Index Layer: Folded Trigram Storage

During index construction, the `extract_trigrams_with_folded()` function **stores both the original case and the lowercase-normalized versions** of the trigram hash for each 3-byte sliding window:

```rust
pub fn extract_trigrams_with_folded(data: &[u8]) -> HashSet<u64> {
    let mut trigrams = HashSet::new();
    for window in data.windows(3) {
        if window.contains(&b'\n') { continue; }
        // Original case
        trigrams.insert(fnv1a_hash(window));
        // Lowercase normalized
        let folded: [u8; 3] = [
            window[0].to_ascii_lowercase(),
            window[1].to_ascii_lowercase(),
            window[2].to_ascii_lowercase(),
        ];
        trigrams.insert(fnv1a_hash(&folded));
    }
    trigrams
}
```

For example, when a file contains `"HashMap"`, the index stores both:
- Original trigrams: `hash("Has")`, `hash("ash")`, `hash("shM")`, `hash("hMa")`, `hash("Map")`
- Folded trigrams: `hash("has")`, `hash("ash")`, `hash("shm")`, `hash("hma")`, `hash("map")`

Since a `HashSet` is used, trigrams that are already lowercase (e.g., `"ash"`) are not stored redundantly.

### 9.2 Query Layer: Folded Trigram Extraction

The query decomposer `decompose()` accepts a `case_insensitive: bool` parameter and selects the appropriate trigram extraction function based on the flag:

```rust
pub fn decompose(pattern: &str, case_insensitive: bool) -> DecomposedQuery {
    let extract_fn = if case_insensitive {
        extract_literal_trigrams_folded  // Convert to lowercase first, then extract
    } else {
        extract_literal_trigrams         // Extract as-is
    };
    // ... Use extract_fn to extract must_match and alternatives
}
```

The implementation of `extract_literal_trigrams_folded()` is very concise — it converts the literal to ASCII lowercase, then calls the standard extraction:

```rust
pub fn extract_literal_trigrams_folded(s: &str) -> Vec<u64> {
    let lower = s.to_ascii_lowercase();
    extract_literal_trigrams(&lower)
}
```

### 9.3 Complete Flow

```
Search for "hashmap" (-i mode):
  1. decompose("hashmap", case_insensitive=true)
     → extract_literal_trigrams_folded("hashmap")
     → trigrams of "hashmap": [hash("has"), hash("ash"), hash("shm"), hash("hma"), hash("map")]
  2. Look up these folded trigrams in the index → hit (because the index already stores folded versions)
  3. Obtain candidate file set (same index-accelerated path as case-sensitive search)
  4. Run (?i)hashmap regex verification on candidate files
```

**Key improvement**: Case-insensitive search **no longer falls back to a full scan**; instead, it uses folded trigrams to take the index-accelerated path, achieving the same reduction ratio as case-sensitive search.

### 9.4 Regex Verification

Regardless of whether index acceleration is used, the regex matching phase always achieves case-insensitivity by prepending `(?i)`:

```rust
let regex_pattern = if opts.case_insensitive {
    format!("(?i){}", &opts.pattern)
} else {
    opts.pattern.clone()
};
```

---

## 10. Git Integration and Index Freshness

### 10.1 Freshness Model

```
At index build time, record the HEAD commit hash → index.meta.commit_hash
At search time, compare current HEAD vs stored commit

Match     → Index is fresh, use directly
Mismatch  → Index is stale, needs rebuilding
```

#### 10.1.1 Non-Git Directory Handling

For directories not in a Git repository, detection is done via `is_git_repo()`:

```rust
pub fn is_git_repo(root: &Path) -> bool {
    gix::discover(root).is_ok()
}
```

When the directory is not a Git repository, `is_index_fresh()` always returns `true`, trusting the existing index. However, **mtime-based delta detection** is used to catch file changes between searches.

At index build time, `SystemTime::now()` is recorded as `build_timestamp` (epoch seconds) in `index.meta`. At search time, `detect_fs_changes()` walks the directory and:
- Files with mtime > build_timestamp → treated as modified/new
- Indexed files missing from disk → treated as deleted

```rust
pub fn detect_fs_changes(
    root: &Path,
    indexed_files: &[String],
    build_timestamp: u64,
) -> Result<(Vec<String>, Vec<String>)> {
    let build_time = UNIX_EPOCH + Duration::from_secs(build_timestamp);

    // Walk directory, stat each file
    for entry in walker {
        if entry.metadata()?.modified()? > build_time {
            modified.push(rel_path);
        }
        current_files.insert(rel_path);
    }

    // Detect deletions
    let deleted = indexed_files.iter()
        .filter(|f| !current_files.contains(f.as_str()))
        .collect();

    Ok((modified, deleted))
}
```

This is lightweight — `stat()` is much cheaper than `read()`. Only the changed files have their content read and searched via the existing `DeltaLayer`.

### 10.2 Auto-Rebuild and Delta Layer Integration

The complete flow for the search command (`search.rs`):

```rust
// 1. Check index freshness, full rebuild if necessary
if auto_index && !git::is_index_fresh(root, reader.commit_hash()) {
    eprintln!("Index is stale, rebuilding...");
    build_index(&opts)?;
    reader = IndexReader::open(root)?;
}

// 2. Build the delta layer (overlay for uncommitted changes)
let delta = build_delta_layer(root);

// 3. Execute search (passing in the delta layer)
execute_search(&reader, &search_opts, delta.as_ref())?;
```

`build_delta_layer()` branches based on the directory type:
- **Git repo**: uses `git status`/`git diff-index` for delta detection (unchanged)
- **Non-git directory**: calls `detect_fs_changes()` with the index's `build_timestamp` and file list, then builds a `DeltaLayer` from the result

The CLI uses the `--no-auto-index` flag (auto-indexing is enabled by default) to control whether automatic index building/refreshing is allowed:

```
fastgrep search "pattern"              # Default: auto-index + delta
fastgrep search "pattern" --no-auto-index  # Skip auto-indexing
```

### 10.3 Change Detection

Changes are detected via `git status --porcelain` and `git diff-index`:

```
git diff-index --name-status <stored_commit>
→ M	src/lib.rs          (modified)
→ A	src/new_file.rs     (added)
→ D	src/old_file.rs     (deleted)

git ls-files --others --exclude-standard
→ untracked_file.rs     (untracked)
```

### 10.4 Delta Layer Implementation

`DeltaLayer` provides an overlay for uncommitted changes, **fully integrated into the search pipeline**:

```rust
pub struct DeltaLayer {
    // Added/modified files → re-extracted trigram sets
    pub modified_trigrams: BTreeMap<String, HashSet<u64>>,
    // Paths of deleted files
    pub deleted_files: HashSet<String>,
}
```

`execute_search()` accepts an `Option<&DeltaLayer>` parameter. The search flow:

1. **Main index query**: Normal trigram lookup to obtain the candidate file set
2. **Exclude deleted files**: Filter out files in `delta.deleted_files` from the candidates
3. **Search delta files**: Iterate over added/modified files in `delta.modified_trigrams` and perform additional search on files not already covered by the main index candidates
4. **Merge results**: Combine results from the main index with delta search results

```rust
// Exclude deleted files
let deleted_files: HashSet<&str> = match delta {
    Some(d) => d.deleted_files.iter().map(|s| s.as_str()).collect(),
    None => HashSet::new(),
};

// Skip deleted files during main index search
for &file_id in &candidate_ids {
    let rel_path = reader.file_path(file_id)?;
    if deleted_files.contains(rel_path) { continue; }
    // ... Execute regex verification
}

// Delta layer: search added/modified files
if let Some(delta) = delta {
    for path in delta.modified_trigrams.keys() {
        if searched_files.contains(path.as_str()) { continue; }
        // ... Execute regex verification on delta files
    }
}
```

`SearchResult` includes a `delta_files` field that reports the number of files additionally searched via the delta layer.

### 10.5 Incremental Index Rebuild

For large directories (e.g., QQMail with 757k files, 6-minute full rebuild), full index reconstruction after every change is impractical. The incremental rebuild mechanism avoids re-reading unchanged files.

#### 10.5.1 Core Principle

Incremental rebuild ≠ in-place patching of on-disk files (posting list size changes would shift all offsets). Instead:

```
Load old index posting lists → remap file IDs → re-extract only changed files → merge → rewrite index
```

The key optimization: **skip reading 99%+ of file contents**. Only the changed/new files are read and re-processed.

#### 10.5.2 Algorithm

```
incremental_rebuild(opts) → Result<Option<BuildStats>>:
  1. Open old IndexReader, get build_timestamp
  2. Detect changes:
     - Git repo: changed_files_since(stored_commit)
     - Non-git: detect_fs_changes(build_timestamp)
  3. If no changes → return None
  4. If change ratio > 20% → fall back to full build_index()
  5. discover_files() → new file list, build path→new_id mapping
  6. Build old_id→new_id mapping (skip deleted, mark modified)
  7. Iterate all old lookup entries:
     - Decode each posting list
     - Remap old file IDs → new file IDs
     - Skip deleted/modified files (modified will be re-extracted)
     - Build new trigram_map
  8. Parallel extract trigrams for changed/new files only (rayon)
  9. Merge new trigrams into trigram_map
  10. Remap to contiguous IDs, write_index()
```

#### 10.5.3 Trigger Mechanisms

Two trigger paths:

| Trigger | When | Source |
|---------|------|--------|
| **Manual** | `fastgrep index --incremental` | User invokes CLI |
| **Auto (search-time)** | Delta files ≥ 100 | `build_delta_layer()` in `search.rs` |
| **Auto (stale index)** | HEAD commit mismatch | `run()` in `search.rs` |

Auto-trigger flow in `search.rs`:

```rust
// Stale index → incremental rebuild instead of full rebuild
if auto_index && !git::is_index_fresh(root, reader.commit_hash()) {
    incremental_rebuild(&opts)?;  // falls back to full rebuild on failure
}

// Delta too large → incremental rebuild
if delta_count >= INCREMENTAL_REBUILD_THRESHOLD {
    incremental_rebuild(&opts)?;
}
```

#### 10.5.4 Safety Mechanisms

| Condition | Action |
|-----------|--------|
| No changes detected | Return `None`, skip rebuild |
| Change ratio > 20% | Fall back to full `build_index()` |
| No `build_timestamp` in old index | Fall back to full `build_index()` |
| Incremental rebuild fails | Fall back to full rebuild or continue with delta layer |

#### 10.5.5 Performance

For a 757k-file directory (QQMail):

| Scenario | Full Rebuild | Incremental Rebuild |
|----------|-------------|-------------------|
| Time | ~6 minutes | Seconds (proportional to changed files) |
| Files read | 757,000 | Only changed files |
| Bottleneck | Reading all file contents | Directory stat walk + reading changed files |

---

## 11. Context Line Handling

### 11.1 Algorithm

```rust
fn search_file(path, rel_path, regex, before_ctx, after_ctx) -> Vec<SearchMatch> {
    let lines: Vec<String> = read_all_lines(file);
    let mut context_lines_added: HashSet<usize> = HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            // 1. Add before-context
            for ctx_i in i.saturating_sub(before_ctx)..i {
                if context_lines_added.insert(ctx_i) { /* add */ }
            }

            // 2. Add the matching line itself
            if context_lines_added.insert(i) { /* add */ }

            // 3. Add after-context
            for ctx_i in (i+1)..(i+after_ctx+1).min(lines.len()) {
                if context_lines_added.insert(ctx_i) { /* add */ }
            }
        }
    }
}
```

### 11.2 Deduplication Mechanism

A `HashSet<usize>` tracks which line numbers have already been added. When context lines from multiple matches overlap, this ensures each line is output only once.

---

## 12. File Filtering

### 12.1 File Type Filtering (`-t`)

Filters candidates by extension:

```rust
if let Some(ref ft) = file_type {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != ft.as_str() {
        return false;
    }
}
```

Example: `-t rs` keeps only `.rs` files.

### 12.2 Glob Filtering (`-g`)

Uses the `globset` crate for glob matching:

```rust
let glob_matcher = globset::Glob::new(pattern)?.compile_matcher();
if !glob_matcher.is_match(path) {
    return false;
}
```

Example: `-g "*.tsx"` keeps only `.tsx` files.

### 12.3 Filtering Timing

Filtering is executed **after** the index lookup and **before** full-text verification, reducing unnecessary file I/O:

```
Index lookup → candidate ID list → type/glob filtering → refined candidates → full regex verification
```

---

## 13. Weight System (Sparse N-gram Reserved)

### 13.1 Byte-Pair Frequency Table

```rust
pub struct PairFrequencyTable {
    counts: Vec<u64>,   // 256 × 256 = 65536 entries
    total: u64,
}
```

Computes the occurrence frequency of all adjacent byte pairs from a corpus.

### 13.2 Selectivity Scoring

```rust
pub fn ngram_selectivity(&self, bytes: &[u8]) -> f64 {
    // Take the minimum frequency among all byte pairs in the n-gram (bottleneck method)
    bytes.windows(2)
        .map(|w| self.frequency(w[0], w[1]))
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0)
}
```

The minimum-frequency pair determines the selectivity of the entire n-gram: lower frequency → rarer → better filtering effectiveness.

### 13.3 CRC32 Weight

```rust
pub fn crc32_weight(pair: &[u8; 2]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(pair);
    hasher.finalize()
}
```

Used for sparse n-gram selection: for longer literals, instead of extracting all trigrams, a subset of variable-length n-grams with the highest CRC32 weight (estimated to be the rarest) is selected. This feature has a reserved interface; the current MVP uses fixed trigrams.

---

## 14. Output Formatting

### 14.1 Terminal Color Detection

```rust
fn supports_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;        // Respect the NO_COLOR protocol
    }
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}
```

- Respects the [NO_COLOR](https://no-color.org/) environment variable
- Uses the Rust standard library `IsTerminal` trait to detect whether stdout is connected to a TTY

### 14.2 ANSI Color Scheme

| Element | ANSI Sequence | Color |
|------|-----------|------|
| Filename | `\x1b[35m...\x1b[0m` | Magenta |
| Line number | `\x1b[32m...\x1b[0m` | Green |
| Match content | No special coloring | Default |

Consistent with ripgrep's color scheme.

---

## 15. Performance Analysis

### 15.1 Theoretical Complexity

| Operation | Time Complexity | Description |
|------|-----------|------|
| Index construction | O(F × L) | F = number of files, L = average file length |
| Trigram lookup | O(log N) | N = number of unique trigrams |
| Posting decoding | O(P) | P = posting list length |
| Intersection of k trigrams | O(k × P_min) | P_min = size of smallest posting list |
| Full-text verification | O(C × L) | C = number of candidate files, L = file length |

### 15.2 Actual Test Data

Search results on fastgrep's own repository (2244 files):

| Pattern | Candidates/Total | Reduction Ratio |
|------|----------|--------|
| `"HashMap"` | 6 / 2244 | 374× |
| `"impl.*Display"` | 4 / 2244 | 561× |
| `".*"` (not optimizable) | 2244 / 2244 | 1× (full scan) |

### 15.3 Release Build Optimization

```toml
[profile.release]
lto = true          # Cross-crate link-time optimization
codegen-units = 1   # Single codegen unit for more aggressive optimization
strip = true        # Strip debug symbols to reduce binary size
```

### 15.4 Verification Phase Performance Optimization

The index lookup phase (trigram → candidate file IDs) is already very fast (microsecond level). The actual bottleneck is the **verification phase** — running full regex matching on candidate files. Before optimization, the verification phase accounted for 70-90% of total search time.

#### Optimization 1: mmap Replaces BufReader (Zero-Copy File Reading)

**Before:**
```rust
let file = std::fs::File::open(path)?;
let reader = BufReader::new(file);
let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
// Problems:
// 1. Allocates a String per line (heap allocation)
// 2. Materializes all lines before matching starts
// 3. BufReader syscall overhead
```

**After:**
```rust
let file = std::fs::File::open(path)?;
let mmap = unsafe { Mmap::map(&file)? };
let data = &mmap[..];  // Zero-copy: OS loads pages on demand

// Scan line start offsets directly on &[u8]
let line_starts = find_line_starts(data);

// Match line by line without allocating Strings
for line_idx in 0..line_count {
    if regex.is_match(line_bytes(line_idx)) { ... }
}
```

**Key improvements:**
- Zero-copy file content (OS page cache mapped directly to user space)
- No per-line String allocation; `from_utf8_lossy` only runs on output lines
- Sentinel trick (`line_starts` ends with `data.len()`) unifies line-end calculation

#### Optimization 2: Bytes Regex Replaces String Regex

**Before:**
```rust
use regex::Regex;  // Requires &str input (UTF-8)
// Must convert &[u8] to String first → one UTF-8 decode per line
```

**After:**
```rust
use regex::bytes::Regex as BytesRegex;  // Matches directly on &[u8]
// mmap's &[u8] passed directly, skipping UTF-8 decode overhead
```

UTF-8 validation/decoding has non-trivial overhead on large files; bytes regex skips it entirely.

#### Optimization 3: Rayon Parallel File Verification

**Before:**
```rust
for &file_id in &candidate_ids {
    let matches = search_file(...);  // Sequential
    all_matches.extend(matches);
}
```

**After:**
```rust
let matches: Vec<SearchMatch> = candidate_paths
    .par_iter()  // Rayon auto-distributes across all CPU cores
    .flat_map(|(rel_path, full_path)| {
        search_file_mmap(full_path, rel_path, &regex, ...)
            .unwrap_or_default()
    })
    .collect();
```

For patterns with many candidate files (e.g., alternation), multi-core parallelism delivers near-linear speedup.

#### Optimization 4: Direct Byte Reads for Index Binary Search

**Before:**
```rust
fn read_lookup_entry(&self, data: &[u8], index: usize) -> LookupEntry {
    let mut cursor = std::io::Cursor::new(entry_bytes);  // Cursor allocation per comparison
    LookupEntry::read_from(&mut cursor)
}
```

**After:**
```rust
#[inline]
fn read_lookup_entry(&self, data: &[u8], index: usize) -> LookupEntry {
    let bytes = &data[offset..offset + 16];
    let ngram_hash = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let offset = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    LookupEntry { ngram_hash, offset, len }
}
```

Eliminates Cursor allocation + trait dispatch; pure pointer arithmetic.

### 15.5 Measured Speedup Data

#### Linux Kernel (92,790 indexed files, cold cache)

Cold cache tests (page cache dropped via `echo 3 > /proc/sys/vm/drop_caches` before each run) represent the realistic scenario where an Agent searches a large codebase for the first time:

| Pattern | rg | fastgrep | Speedup |
|---------|-----|----------|---------|
| `KASAN_SHADOW_OFFSET` (rare, 61 matches) | 21.2s | 0.52s | **41x** |
| `HashMap` (rare, 15 matches) | 19.8s | 0.30s | **66x** |

#### Linux Kernel (92,790 indexed files, warm cache)

With all files in page cache, rg scans 92k files in ~160ms. fastgrep's process startup + meta JSON parsing overhead dominates:

| Pattern | rg | fastgrep | Speedup |
|---------|-----|----------|---------|
| `KASAN_SHADOW_OFFSET` (rare) | 158ms | 188ms | 0.8x |
| `HashMap` (rare) | 163ms | 182ms | 0.9x |
| `EXPORT_SYMBOL` (common, 40k matches) | 174ms | 421ms | 0.4x |
| `impl\s+\w+\s+for\s+\w+` (regex) | 167ms | 865ms | 0.2x |

#### Speedup Patterns

| Factor | Effect |
|--------|--------|
| **Cold cache** (realistic) | **41-66x** — fastgrep only reads index + candidate files; rg must read all 92k files |
| **Warm cache** (all in RAM) | 0.2-0.9x — rg's SIMD-optimized mmap scan is hard to beat when I/O is free |
| **More files** | Greater cold-cache speedup (index eliminates more I/O) |
| **Rarer pattern** | Greater speedup (fewer candidates → less verification I/O) |
| **Non-optimizable pattern (`.*`)** | No speedup (full scan fallback) |

**Key insight**: fastgrep's primary advantage is **I/O reduction**. By reading only the index file + a handful of candidate files instead of all 92k files, it achieves dramatic speedups when disk/page-cache is the bottleneck. When all files are already in RAM (warm cache), rg's raw scanning speed with SIMD is difficult to surpass.

---

## 16. Testing Strategy

### 16.1 Test Layering

| Level | Count | Coverage |
|------|------|---------|
| Unit tests | 20 | Hash determinism, varint encode/decode, format serialization, trigram extraction, query decomposition, case-insensitive decomposition |
| Integration tests | 12 | End-to-end build + search, regex, alternation, case sensitivity, file filtering, context lines, full scan fallback, delta layer added files, delta layer deleted file exclusion, non-git mtime delta detection, incremental rebuild (add/modify/delete), incremental rebuild no-changes |
| Correctness tests | 6 | 24 patterns against naive grep line-by-line comparison, no false negatives verification, case-insensitive accuracy, context line correctness, file type filter correctness, edge case patterns |
| **Total** | **38** | |

### 16.2 Key Test Cases

**Varint boundary value tests**:
```rust
for &val in &[0, 1, 127, 128, 300, 16384, u32::MAX] {
    assert_eq!(decode(encode(val)), val);
}
```

**Index reduction effectiveness verification**:
```rust
assert!(result.candidate_count < result.total_files,
    "index should narrow candidates");
```

**Full scan fallback verification**:
```rust
// r".*" is not optimizable, must fall back to full scan
assert!(!result.used_index);
assert_eq!(result.candidate_count, result.total_files);
```

**Delta layer added file test**:
```rust
// After index build, a newly added file is not found without the delta layer
let result = execute_search(&reader, &search_opts, None).unwrap();
assert!(result.matches.is_empty());

// The new file can be found via the delta layer
let delta = DeltaLayer::from_changed_files(root, &["new_feature.rs".to_string()], &[]).unwrap();
let result = execute_search(&reader, &search_opts, Some(&delta)).unwrap();
assert!(!result.matches.is_empty());
```

**Delta layer deleted file exclusion test**:
```rust
// After deleting a file, the delta layer excludes it from results
let delta = DeltaLayer::from_changed_files(root, &[], &["notes.txt".to_string()]).unwrap();
let result = execute_search(&reader, &search_opts, Some(&delta)).unwrap();
assert!(!result.matches.iter().any(|m| m.file == "notes.txt"));
```

### 16.3 Test Corpus

Integration tests use `tempfile` to create 4 test files (Rust, Python, text) in a temporary directory, verifying the correctness of cross-language search.

---

## 17. Benchmark Framework

### 17.1 Test Matrix

| Dimension | Values |
|------|------|
| Corpus | small (100 files), medium (10k files), linux-kernel |
| Pattern type | Literal (common/rare/medium), regex (function declaration/import/impl trait/TODO), not optimizable |
| Iterations | Configurable, default 10 iterations taking the median |

### 17.2 Test Patterns

```
literal_common:     "fn"
literal_rare:       "SPDX-License-Identifier"
literal_medium:     "HashMap"
regex_fn_decl:      r"fn\s+\w+\s*\("
regex_use_stmt:     r"use\s+\w+::\w+"
regex_impl_trait:   r"impl\s+\w+\s+for\s+\w+"
regex_todo:         r"(TODO|FIXME|HACK)\b"
regex_dot_star:     ".*"
```

### 17.3 Output Format

CSV raw data + Markdown report tables:

```
| Pattern          | rg (ms) | fastgrep (ms) | Speedup | Matches |
|------------------|---------|---------------|---------|---------|
| literal_rare     | 138.0   | 2.0           | 69.0x   | 12      |
| regex_impl_trait | 487.0   | 8.0           | 60.9x   | 203     |
| regex_dot_star   | 156.0   | 165.0         | 0.9x    | 74000   |
```

---

## 18. Future Evolution Directions

### Phase 2: Performance Optimization
- [ ] Sparse n-gram: Select variable-length n-grams based on byte-pair frequency
- [ ] Complete regex AST traversal (currently only handles Literal/Concat/Alternation)
- [x] ~~Index-accelerated case-insensitive search (lowercase-normalized index)~~ ✅ Done: Stores folded trigrams at build time, uses folded extraction at query time
- [x] ~~mmap + bytes regex for file verification~~ ✅ Done: Zero-copy file reading, regex matches directly on `&[u8]`
- [x] ~~Parallel file verification~~ ✅ Done: Rayon `par_iter` for multi-core candidate file search
- [x] ~~Direct byte reads for index binary search~~ ✅ Done: Eliminates Cursor allocation overhead

### Phase 3: Incremental Updates
- [x] ~~Delta layer actually integrated into the search pipeline~~ ✅ Done: `execute_search` accepts `Option<&DeltaLayer>`, excludes deleted files, searches added/modified files
- [x] ~~Non-git directory delta detection via filesystem mtime~~ ✅ Done: Records `build_timestamp` in `index.meta`, walks directory at search time to detect new/modified/deleted files
- [x] ~~Incremental index updates (process only changed files, avoiding full rebuilds)~~ ✅ Done: `incremental_rebuild()` loads old posting lists, remaps file IDs, re-extracts only changed files, merges, rewrites index. Auto-triggered when delta exceeds threshold or index is stale. Manual via `fastgrep index --incremental`.

### Phase 4: Agent Deep Integration
- [ ] MCP Server mode (persistent process, avoiding repeated mmap overhead)
- [ ] Search result ranking (sorted by relevance)
- [ ] Parallel multi-pattern search (query multiple patterns at once)
