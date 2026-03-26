/// Index writer: serialize the in-memory index to the three-file format.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::index::format::*;
use crate::index::posting;
use crate::INDEX_DIR;

/// Write the complete index to disk.
///
/// `trigram_map`: maps ngram_hash → sorted Vec of file IDs
/// `files`: ordered list of file paths (index = file ID)
/// `root`: the repository root where .fastgrep/ will be created
/// `commit_hash`: optional HEAD commit hash
pub fn write_index(
    trigram_map: &BTreeMap<u64, Vec<u32>>,
    files: &[String],
    root: &Path,
    commit_hash: Option<String>,
) -> Result<()> {
    let index_dir = root.join(INDEX_DIR);
    fs::create_dir_all(&index_dir).context("creating index directory")?;

    let postings_path = index_dir.join(POSTINGS_FILE);
    let lookup_path = index_dir.join(LOOKUP_FILE);
    let meta_path = index_dir.join(META_FILE);

    // 1. Write postings file
    let mut postings_writer = BufWriter::new(
        fs::File::create(&postings_path).context("creating postings file")?,
    );
    let postings_header = FileHeader::new(POSTINGS_MAGIC);
    postings_header.write_to(&mut postings_writer)?;

    // Track offset for each ngram's posting list
    let mut lookup_entries: Vec<LookupEntry> = Vec::with_capacity(trigram_map.len());
    let mut current_offset = HEADER_SIZE as u32;

    for (&ngram_hash, file_ids) in trigram_map {
        let encoded = posting::encode_posting_list(file_ids);
        let len = encoded.len() as u32;
        postings_writer.write_all(&encoded)?;

        lookup_entries.push(LookupEntry {
            ngram_hash,
            offset: current_offset,
            len,
        });
        current_offset += len;
    }
    postings_writer.flush()?;

    // 2. Write lookup file (sorted by ngram_hash for binary search)
    lookup_entries.sort_by_key(|e| e.ngram_hash);
    let mut lookup_writer = BufWriter::new(
        fs::File::create(&lookup_path).context("creating lookup file")?,
    );
    let lookup_header = FileHeader::new(LOOKUP_MAGIC);
    lookup_header.write_to(&mut lookup_writer)?;

    for entry in &lookup_entries {
        entry.write_to(&mut lookup_writer)?;
    }
    lookup_writer.flush()?;

    // 3. Write metadata
    let meta = IndexMeta {
        version: FORMAT_VERSION,
        file_count: files.len() as u32,
        trigram_count: lookup_entries.len() as u32,
        commit_hash,
        files: files.to_vec(),
    };
    let meta_json = serde_json::to_string_pretty(&meta)?;
    fs::write(&meta_path, meta_json).context("writing meta file")?;

    Ok(())
}
