pub mod ngram;
pub mod index;
pub mod query;
pub mod git;

/// The directory name where fastgrep stores its index files.
pub const INDEX_DIR: &str = ".fastgrep";
