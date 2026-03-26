/// Query planning: sort trigrams by selectivity for optimal intersection order.

use crate::index::reader::IndexReader;

/// A query plan: ordered list of trigram hashes to look up.
#[derive(Debug)]
pub struct QueryPlan {
    /// Trigram hashes sorted by estimated posting list size (smallest first).
    pub ordered_trigrams: Vec<u64>,
    /// Alternative groups (for alternation patterns).
    pub alternative_groups: Vec<Vec<u64>>,
    /// Whether the query can use the index.
    pub uses_index: bool,
}

/// Create a query plan by sorting trigrams by their posting list size.
pub fn plan_query(
    must_match: &[u64],
    alternatives: &[Vec<u64>],
    reader: &IndexReader,
) -> QueryPlan {
    if must_match.is_empty() && alternatives.is_empty() {
        return QueryPlan {
            ordered_trigrams: Vec::new(),
            alternative_groups: Vec::new(),
            uses_index: false,
        };
    }

    // Sort must_match trigrams by posting list size (smallest first for early termination)
    let mut scored: Vec<(u64, u32)> = must_match
        .iter()
        .map(|&hash| {
            let size = reader.posting_size(hash).unwrap_or(u32::MAX);
            (hash, size)
        })
        .collect();
    scored.sort_by_key(|&(_, size)| size);

    let ordered_trigrams: Vec<u64> = scored.into_iter().map(|(hash, _)| hash).collect();

    // For alternatives, sort each group similarly
    let alternative_groups: Vec<Vec<u64>> = alternatives
        .iter()
        .map(|group| {
            let mut scored: Vec<(u64, u32)> = group
                .iter()
                .map(|&hash| {
                    let size = reader.posting_size(hash).unwrap_or(u32::MAX);
                    (hash, size)
                })
                .collect();
            scored.sort_by_key(|&(_, size)| size);
            scored.into_iter().map(|(hash, _)| hash).collect()
        })
        .collect();

    QueryPlan {
        ordered_trigrams,
        alternative_groups,
        uses_index: true,
    }
}
