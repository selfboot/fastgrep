/// N-gram extraction: trigram sliding window with FNV-1a hashing.
use std::collections::HashSet;

/// FNV-1a hash for a trigram (3 bytes).
#[inline]
pub fn fnv1a_hash(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A single n-gram with its hash and source bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ngram {
    pub hash: u64,
    pub bytes: Vec<u8>,
}

/// Extract all trigrams from a byte slice.
/// Returns a set of unique trigram hashes.
pub fn extract_trigrams(data: &[u8]) -> HashSet<u64> {
    let mut trigrams = HashSet::new();
    if data.len() < 3 {
        return trigrams;
    }
    for window in data.windows(3) {
        // Skip trigrams that span a newline — they're not useful for search
        if window.contains(&b'\n') {
            continue;
        }
        trigrams.insert(fnv1a_hash(window));
    }
    trigrams
}

/// Extract all trigrams from a byte slice, including lowercase-normalized copies.
/// Used during index building so that case-insensitive queries can match.
pub fn extract_trigrams_with_folded(data: &[u8]) -> HashSet<u64> {
    let mut trigrams = HashSet::new();
    if data.len() < 3 {
        return trigrams;
    }
    for window in data.windows(3) {
        if window.contains(&b'\n') {
            continue;
        }
        // Original case
        trigrams.insert(fnv1a_hash(window));
        // Lowercase-folded
        let folded: [u8; 3] = [
            window[0].to_ascii_lowercase(),
            window[1].to_ascii_lowercase(),
            window[2].to_ascii_lowercase(),
        ];
        trigrams.insert(fnv1a_hash(&folded));
    }
    trigrams
}

/// Extract trigrams from a byte slice, returning (hash, trigram_bytes) pairs.
pub fn extract_trigrams_with_bytes(data: &[u8]) -> Vec<Ngram> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    if data.len() < 3 {
        return result;
    }
    for window in data.windows(3) {
        if window.contains(&b'\n') {
            continue;
        }
        let hash = fnv1a_hash(window);
        if seen.insert(hash) {
            result.push(Ngram {
                hash,
                bytes: window.to_vec(),
            });
        }
    }
    result
}

/// Extract trigram hashes from a string literal (for query decomposition).
pub fn extract_literal_trigrams(s: &str) -> Vec<u64> {
    let bytes = s.as_bytes();
    if bytes.len() < 3 {
        return Vec::new();
    }
    let mut trigrams = Vec::new();
    let mut seen = HashSet::new();
    for window in bytes.windows(3) {
        let hash = fnv1a_hash(window);
        if seen.insert(hash) {
            trigrams.push(hash);
        }
    }
    trigrams
}

/// Extract lowercase-folded trigram hashes from a string literal.
/// Used for case-insensitive query decomposition.
pub fn extract_literal_trigrams_folded(s: &str) -> Vec<u64> {
    let lower = s.to_ascii_lowercase();
    extract_literal_trigrams(&lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = fnv1a_hash(b"abc");
        let h2 = fnv1a_hash(b"abc");
        assert_eq!(h1, h2);
        assert_ne!(fnv1a_hash(b"abc"), fnv1a_hash(b"abd"));
    }

    #[test]
    fn test_extract_trigrams() {
        let data = b"Hello";
        let trigrams = extract_trigrams(data);
        // "Hel", "ell", "llo" = 3 trigrams
        assert_eq!(trigrams.len(), 3);
    }

    #[test]
    fn test_extract_short_input() {
        assert!(extract_trigrams(b"ab").is_empty());
        assert!(extract_trigrams(b"").is_empty());
    }

    #[test]
    fn test_skip_newlines() {
        let data = b"ab\ncd";
        let trigrams = extract_trigrams(data);
        // "ab\n", "b\nc", "\ncd" all contain newlines — should be empty
        assert!(trigrams.is_empty());
    }

    #[test]
    fn test_literal_trigrams() {
        let tris = extract_literal_trigrams("HashMap");
        // "Has", "ash", "shM", "hMa", "Map" = 5
        assert_eq!(tris.len(), 5);
    }
}
