/// Query decomposition: extract trigrams from regex patterns.
///
/// Uses regex-syntax HIR to walk the AST and extract literal substrings,
/// then generates trigrams from those literals.

use regex_syntax::hir::{Hir, HirKind};

use crate::ngram::extract::{extract_literal_trigrams, extract_literal_trigrams_folded};

/// Result of decomposing a regex into trigram queries.
#[derive(Debug)]
pub struct DecomposedQuery {
    /// Trigrams that ALL must match (conjunction).
    pub must_match: Vec<u64>,
    /// Groups of trigrams where at least one group must match (disjunction of conjunctions).
    /// Used for alternation: `(foo|bar)` → [[trigrams(foo)], [trigrams(bar)]]
    pub alternatives: Vec<Vec<u64>>,
    /// Whether the pattern can be optimized with trigrams.
    pub optimizable: bool,
}

/// Decompose a regex pattern into trigram requirements.
/// If `case_insensitive` is true, uses lowercase-folded trigrams
/// (requires index built with folded trigrams).
pub fn decompose(pattern: &str, case_insensitive: bool) -> DecomposedQuery {
    let hir = match regex_syntax::parse(pattern) {
        Ok(hir) => hir,
        Err(_) => {
            return DecomposedQuery {
                must_match: Vec::new(),
                alternatives: Vec::new(),
                optimizable: false,
            };
        }
    };

    let extract_fn = if case_insensitive {
        extract_literal_trigrams_folded
    } else {
        extract_literal_trigrams
    };

    let mut must_match = Vec::new();
    let mut alternatives = Vec::new();
    let optimizable;

    // Extract literals from the HIR
    let literals = extract_literals_from_hir(&hir);

    match literals {
        LiteralExtraction::Exact(s) => {
            must_match = extract_fn(&s);
            optimizable = !must_match.is_empty();
        }
        LiteralExtraction::Conjunction(parts) => {
            for part in &parts {
                must_match.extend(extract_fn(part));
            }
            // Deduplicate
            must_match.sort_unstable();
            must_match.dedup();
            optimizable = !must_match.is_empty();
        }
        LiteralExtraction::Alternation(alts) => {
            for alt in &alts {
                let trigrams = extract_fn(alt);
                if !trigrams.is_empty() {
                    alternatives.push(trigrams);
                }
            }
            optimizable = !alternatives.is_empty();
        }
        LiteralExtraction::None => {
            optimizable = false;
        }
    }

    DecomposedQuery {
        must_match,
        alternatives,
        optimizable,
    }
}

/// What we extracted from the HIR.
#[derive(Debug)]
enum LiteralExtraction {
    /// A single exact literal string.
    Exact(String),
    /// Multiple literal parts that all must match (from concatenation).
    Conjunction(Vec<String>),
    /// Alternatives (from alternation) — at least one must match.
    Alternation(Vec<String>),
    /// No useful literals found.
    None,
}

/// Recursively extract literal strings from a HIR node.
fn extract_literals_from_hir(hir: &Hir) -> LiteralExtraction {
    match hir.kind() {
        HirKind::Literal(lit) => {
            if let Ok(s) = std::str::from_utf8(&lit.0) {
                LiteralExtraction::Exact(s.to_string())
            } else {
                LiteralExtraction::None
            }
        }
        HirKind::Concat(parts) => {
            let mut literals = Vec::new();
            for part in parts {
                match extract_literals_from_hir(part) {
                    LiteralExtraction::Exact(s) => literals.push(s),
                    LiteralExtraction::Conjunction(parts) => literals.extend(parts),
                    _ => {} // Skip non-literal parts but keep other literals
                }
            }
            if literals.is_empty() {
                LiteralExtraction::None
            } else if literals.len() == 1 {
                LiteralExtraction::Exact(literals.into_iter().next().unwrap())
            } else {
                LiteralExtraction::Conjunction(literals)
            }
        }
        HirKind::Alternation(alts) => {
            let mut alt_literals = Vec::new();
            for alt in alts {
                match extract_literals_from_hir(alt) {
                    LiteralExtraction::Exact(s) => alt_literals.push(s),
                    LiteralExtraction::Conjunction(parts) => {
                        // Join conjunction parts for the alternation branch
                        alt_literals.push(parts.join(""));
                    }
                    _ => {
                        // If any branch has no literals, alternation is not optimizable
                        return LiteralExtraction::None;
                    }
                }
            }
            if alt_literals.is_empty() {
                LiteralExtraction::None
            } else {
                LiteralExtraction::Alternation(alt_literals)
            }
        }
        HirKind::Capture(cap) => extract_literals_from_hir(&cap.sub),
        HirKind::Repetition(rep) => {
            // For repetitions with min >= 1, we can use the sub-pattern's literals
            if rep.min >= 1 {
                extract_literals_from_hir(&rep.sub)
            } else {
                LiteralExtraction::None
            }
        }
        HirKind::Look(_) | HirKind::Class(_) | HirKind::Empty => LiteralExtraction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_pattern() {
        let q = decompose("HashMap", false);
        assert!(q.optimizable);
        assert!(!q.must_match.is_empty());
    }

    #[test]
    fn test_regex_with_literals() {
        let q = decompose(r"impl\s+Display", false);
        assert!(q.optimizable);
        // Should extract trigrams from "impl" and "Display"
        assert!(!q.must_match.is_empty());
    }

    #[test]
    fn test_alternation() {
        let q = decompose(r"(TODO|FIXME|HACK)", false);
        assert!(q.optimizable);
        assert!(!q.alternatives.is_empty());
    }

    #[test]
    fn test_dot_star_not_optimizable() {
        let q = decompose(r".*", false);
        assert!(!q.optimizable);
    }

    #[test]
    fn test_short_literal_not_optimizable() {
        let q = decompose("ab", false);
        assert!(!q.optimizable); // less than 3 chars → no trigrams
    }

    #[test]
    fn test_case_insensitive_decompose() {
        let q = decompose("HashMap", true);
        assert!(q.optimizable);
        // Should produce lowercase-folded trigrams
        let q_lower = decompose("hashmap", true);
        assert!(q_lower.optimizable);
        // Both should produce the same trigrams since they fold to lowercase
        assert_eq!(q.must_match, q_lower.must_match);
    }
}
