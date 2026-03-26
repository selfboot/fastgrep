/// Disk format definitions for the fastgrep index.
///
/// Index consists of three files:
/// - `index.lookup`    — sorted array of LookupEntry (binary search)
/// - `index.postings`  — varint delta-encoded file ID lists
/// - `index.meta`      — JSON metadata

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{self, Read, Write};

/// Magic bytes for the lookup file: "FGLK"
pub const LOOKUP_MAGIC: [u8; 4] = *b"FGLK";

/// Magic bytes for the postings file: "FGPS"
pub const POSTINGS_MAGIC: [u8; 4] = *b"FGPS";

/// Current format version.
pub const FORMAT_VERSION: u32 = 1;

/// Size of the file header (magic + version).
pub const HEADER_SIZE: usize = 8;

/// Size of each lookup entry in bytes: hash(8) + offset(4) + len(4) = 16
pub const LOOKUP_ENTRY_SIZE: usize = 16;

/// File names within the index directory.
pub const LOOKUP_FILE: &str = "index.lookup";
pub const POSTINGS_FILE: &str = "index.postings";
pub const META_FILE: &str = "index.meta";

/// A single entry in the lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupEntry {
    /// FNV-1a hash of the n-gram.
    pub ngram_hash: u64,
    /// Byte offset into the postings file.
    pub offset: u32,
    /// Length in bytes of the posting list.
    pub len: u32,
}

impl LookupEntry {
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_u64::<LittleEndian>(self.ngram_hash)?;
        w.write_u32::<LittleEndian>(self.offset)?;
        w.write_u32::<LittleEndian>(self.len)?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Self> {
        let ngram_hash = r.read_u64::<LittleEndian>()?;
        let offset = r.read_u32::<LittleEndian>()?;
        let len = r.read_u32::<LittleEndian>()?;
        Ok(Self {
            ngram_hash,
            offset,
            len,
        })
    }
}

/// File header written at the start of lookup and postings files.
#[derive(Debug, Clone)]
pub struct FileHeader {
    pub magic: [u8; 4],
    pub version: u32,
}

impl FileHeader {
    pub fn new(magic: [u8; 4]) -> Self {
        Self {
            magic,
            version: FORMAT_VERSION,
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.magic)?;
        w.write_u32::<LittleEndian>(self.version)?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        let version = r.read_u32::<LittleEndian>()?;
        Ok(Self { magic, version })
    }

    pub fn validate(&self, expected_magic: &[u8; 4]) -> io::Result<()> {
        if &self.magic != expected_magic {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid magic: expected {:?}, got {:?}",
                    expected_magic, self.magic
                ),
            ));
        }
        if self.version != FORMAT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported version: expected {}, got {}",
                    FORMAT_VERSION, self.version
                ),
            ));
        }
        Ok(())
    }
}

/// Index metadata stored as JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexMeta {
    pub version: u32,
    pub file_count: u32,
    pub trigram_count: u32,
    pub commit_hash: Option<String>,
    /// Ordered list of file paths in the index. Index position = file ID.
    pub files: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_lookup_entry_roundtrip() {
        let entry = LookupEntry {
            ngram_hash: 0xdeadbeef12345678,
            offset: 42,
            len: 100,
        };
        let mut buf = Vec::new();
        entry.write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), LOOKUP_ENTRY_SIZE);

        let mut cursor = Cursor::new(&buf);
        let read_back = LookupEntry::read_from(&mut cursor).unwrap();
        assert_eq!(entry, read_back);
    }

    #[test]
    fn test_header_roundtrip() {
        let header = FileHeader::new(LOOKUP_MAGIC);
        let mut buf = Vec::new();
        header.write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), HEADER_SIZE);

        let mut cursor = Cursor::new(&buf);
        let read_back = FileHeader::read_from(&mut cursor).unwrap();
        assert_eq!(read_back.magic, LOOKUP_MAGIC);
        assert_eq!(read_back.version, FORMAT_VERSION);
        read_back.validate(&LOOKUP_MAGIC).unwrap();
    }
}
