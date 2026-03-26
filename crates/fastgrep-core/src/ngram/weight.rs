/// CRC32-based weight calculation for sparse n-gram selection.
/// Higher weight = rarer character pair = more selective n-gram.

use crc32fast::Hasher;

/// Compute CRC32 weight for a byte pair.
/// Used to evaluate how "rare" a character pair is in a corpus.
#[inline]
pub fn crc32_weight(pair: &[u8; 2]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(pair);
    hasher.finalize()
}

/// Character pair frequency table built from a corpus.
/// Maps (byte, byte) pairs to their occurrence count.
pub struct PairFrequencyTable {
    /// 256 * 256 table of pair frequencies.
    counts: Vec<u64>,
    total: u64,
}

impl PairFrequencyTable {
    pub fn new() -> Self {
        Self {
            counts: vec![0u64; 256 * 256],
            total: 0,
        }
    }

    /// Add all byte pairs from a data slice.
    pub fn add_data(&mut self, data: &[u8]) {
        if data.len() < 2 {
            return;
        }
        for window in data.windows(2) {
            let idx = (window[0] as usize) * 256 + (window[1] as usize);
            self.counts[idx] += 1;
            self.total += 1;
        }
    }

    /// Get frequency of a byte pair (0.0 to 1.0).
    pub fn frequency(&self, a: u8, b: u8) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let idx = (a as usize) * 256 + (b as usize);
        self.counts[idx] as f64 / self.total as f64
    }

    /// Score an n-gram: lower frequency pairs → higher score (more selective).
    /// Returns the minimum pair frequency in the n-gram (bottleneck approach).
    pub fn ngram_selectivity(&self, bytes: &[u8]) -> f64 {
        if bytes.len() < 2 {
            return 0.0;
        }
        let mut min_freq = f64::MAX;
        for window in bytes.windows(2) {
            let freq = self.frequency(window[0], window[1]);
            if freq < min_freq {
                min_freq = freq;
            }
        }
        min_freq
    }
}

impl Default for PairFrequencyTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_weight() {
        let w1 = crc32_weight(&[b'a', b'b']);
        let w2 = crc32_weight(&[b'a', b'b']);
        assert_eq!(w1, w2);
    }

    #[test]
    fn test_pair_frequency() {
        let mut table = PairFrequencyTable::new();
        table.add_data(b"aabb");
        // pairs: "aa", "ab", "bb"
        assert!(table.frequency(b'a', b'a') > 0.0);
        assert!(table.frequency(b'a', b'b') > 0.0);
        assert!(table.frequency(b'b', b'b') > 0.0);
        assert_eq!(table.frequency(b'x', b'y'), 0.0);
    }
}
