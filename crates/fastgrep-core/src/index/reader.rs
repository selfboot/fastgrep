/// Index reader: memory-mapped lookup table with binary search.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use memmap2::Mmap;

use crate::index::format::*;
use crate::index::posting;
use crate::INDEX_DIR;

/// A read-only handle to an on-disk index.
pub struct IndexReader {
    /// Memory-mapped lookup table.
    lookup_mmap: Mmap,
    /// Memory-mapped postings file.
    postings_mmap: Mmap,
    /// Number of entries in the lookup table.
    entry_count: usize,
    /// Metadata.
    pub meta: IndexMeta,
}

impl IndexReader {
    /// Open an index from the given root directory.
    pub fn open(root: &Path) -> Result<Self> {
        let index_dir = root.join(INDEX_DIR);

        // Read metadata
        let meta_path = index_dir.join(META_FILE);
        let meta_json = fs::read_to_string(&meta_path).context("reading index metadata")?;
        let meta: IndexMeta = serde_json::from_str(&meta_json).context("parsing index metadata")?;

        // Mmap lookup file
        let lookup_path = index_dir.join(LOOKUP_FILE);
        let lookup_file = fs::File::open(&lookup_path).context("opening lookup file")?;
        let lookup_mmap = unsafe { Mmap::map(&lookup_file).context("mmapping lookup file")? };

        // Validate header
        if lookup_mmap.len() < HEADER_SIZE {
            anyhow::bail!("lookup file too small");
        }
        let mut cursor = std::io::Cursor::new(&lookup_mmap[..HEADER_SIZE]);
        let header = FileHeader::read_from(&mut cursor)?;
        header.validate(&LOOKUP_MAGIC)?;

        let data_len = lookup_mmap.len() - HEADER_SIZE;
        if data_len % LOOKUP_ENTRY_SIZE != 0 {
            anyhow::bail!(
                "lookup file has invalid size: {} data bytes not divisible by {}",
                data_len,
                LOOKUP_ENTRY_SIZE
            );
        }
        let entry_count = data_len / LOOKUP_ENTRY_SIZE;

        // Mmap postings file
        let postings_path = index_dir.join(POSTINGS_FILE);
        let postings_file = fs::File::open(&postings_path).context("opening postings file")?;
        let postings_mmap =
            unsafe { Mmap::map(&postings_file).context("mmapping postings file")? };

        // Validate postings header
        if postings_mmap.len() < HEADER_SIZE {
            anyhow::bail!("postings file too small");
        }
        let mut cursor = std::io::Cursor::new(&postings_mmap[..HEADER_SIZE]);
        let header = FileHeader::read_from(&mut cursor)?;
        header.validate(&POSTINGS_MAGIC)?;

        Ok(Self {
            lookup_mmap,
            postings_mmap,
            entry_count,
            meta,
        })
    }

    /// Binary search the lookup table for an ngram hash.
    /// Returns the posting list (decoded file IDs) if found.
    pub fn lookup(&self, ngram_hash: u64) -> Option<Vec<u32>> {
        let data = &self.lookup_mmap[HEADER_SIZE..];

        // Binary search over sorted lookup entries
        let mut lo = 0usize;
        let mut hi = self.entry_count;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry = self.read_lookup_entry(data, mid);

            match entry.ngram_hash.cmp(&ngram_hash) {
                std::cmp::Ordering::Equal => {
                    return Some(self.read_posting_list(entry.offset, entry.len));
                }
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Get the estimated posting list size for an ngram hash (in bytes).
    /// Used for query planning (smaller = more selective).
    pub fn posting_size(&self, ngram_hash: u64) -> Option<u32> {
        let data = &self.lookup_mmap[HEADER_SIZE..];
        let mut lo = 0usize;
        let mut hi = self.entry_count;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry = self.read_lookup_entry(data, mid);

            match entry.ngram_hash.cmp(&ngram_hash) {
                std::cmp::Ordering::Equal => return Some(entry.len),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Read a lookup entry at the given index. Direct byte reads, no Cursor overhead.
    #[inline]
    fn read_lookup_entry(&self, data: &[u8], index: usize) -> LookupEntry {
        let offset = index * LOOKUP_ENTRY_SIZE;
        let bytes = &data[offset..offset + LOOKUP_ENTRY_SIZE];
        let ngram_hash = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let posting_offset = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        LookupEntry {
            ngram_hash,
            offset: posting_offset,
            len,
        }
    }

    /// Read and decode a posting list from the postings file.
    fn read_posting_list(&self, offset: u32, len: u32) -> Vec<u32> {
        let start = offset as usize;
        let end = start + len as usize;
        if end > self.postings_mmap.len() {
            return Vec::new();
        }
        posting::decode_posting_list(&self.postings_mmap[start..end])
    }

    /// Get the file path for a given file ID.
    pub fn file_path(&self, file_id: u32) -> Option<&str> {
        self.meta.files.get(file_id as usize).map(|s| s.as_str())
    }

    /// Get total number of indexed files.
    pub fn file_count(&self) -> usize {
        self.meta.files.len()
    }

    /// Get the stored commit hash.
    pub fn commit_hash(&self) -> Option<&str> {
        self.meta.commit_hash.as_deref()
    }

    /// Get the build timestamp (epoch seconds).
    pub fn build_timestamp(&self) -> Option<u64> {
        self.meta.build_timestamp
    }
}
