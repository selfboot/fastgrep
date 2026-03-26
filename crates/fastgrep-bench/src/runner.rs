/// Benchmark runner: execute rg vs fastgrep and collect timings.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::Result;

/// Test patterns organized by category.
const PATTERNS: &[(&str, &str)] = &[
    ("literal_common", "fn"),
    ("literal_rare", "SPDX-License-Identifier"),
    ("literal_medium", "HashMap"),
    ("literal_pub_fn", "pub fn new"),
    ("regex_fn_decl", r"fn\s+\w+\s*\("),
    ("regex_use_stmt", r"use\s+\w+::\w+"),
    ("regex_impl_trait", r"impl\s+\w+\s+for\s+\w+"),
    ("regex_todo", r"(TODO|FIXME|HACK)\b"),
    ("regex_dot_star", ".*"),
];

#[derive(Debug, serde::Serialize)]
struct BenchResult {
    pattern_name: String,
    pattern: String,
    tool: String,
    iteration: usize,
    wall_time_ms: f64,
    match_count: usize,
}

/// Find the rg binary. Checks PATH, then common locations.
fn find_rg() -> Result<PathBuf> {
    // Try PATH first
    if let Ok(output) = Command::new("which").arg("rg").output() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() && Path::new(&path).exists() {
            return Ok(PathBuf::from(path));
        }
    }

    // Try common Claude Code vendor location
    let home = std::env::var("HOME").unwrap_or_default();
    let vendor_paths = glob::glob(&format!(
        "{home}/.nvm/versions/node/*/lib/node_modules/*/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-linux/rg"
    ));
    if let Ok(mut paths) = vendor_paths {
        if let Some(Ok(p)) = paths.next() {
            if p.exists() {
                return Ok(p);
            }
        }
    }

    // Hardcoded fallback
    let fallback = PathBuf::from("/usr/bin/rg");
    if fallback.exists() {
        return Ok(fallback);
    }

    anyhow::bail!(
        "ripgrep (rg) not found. Install it or set RG_PATH environment variable.\n\
         Try: cargo install ripgrep"
    )
}

/// Find the fastgrep binary.
fn find_fastgrep() -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let local = PathBuf::from(format!("{home}/.local/bin/fastgrep"));
    if local.exists() {
        return Ok(local);
    }
    // Try PATH
    if let Ok(output) = Command::new("which").arg("fastgrep").output() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() && Path::new(&path).exists() {
            return Ok(PathBuf::from(path));
        }
    }
    anyhow::bail!("fastgrep not found. Run `bash scripts/install.sh` first.")
}

pub fn run(corpus_path: &str, iterations: usize, output: &str) -> Result<()> {
    let corpus = Path::new(corpus_path);
    if !corpus.exists() {
        anyhow::bail!("Corpus path does not exist: {}", corpus_path);
    }

    // Resolve tool paths
    let rg_path = std::env::var("RG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| find_rg().expect("could not find rg"));
    let fg_path = std::env::var("FG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| find_fastgrep().expect("could not find fastgrep"));

    eprintln!("=== Benchmark Configuration ===");
    eprintln!("  Corpus:     {}", corpus_path);
    eprintln!("  Iterations: {}", iterations);
    eprintln!("  rg:         {}", rg_path.display());
    eprintln!("  fastgrep:   {}", fg_path.display());
    eprintln!();

    // Build fastgrep index first (not counted in search timing)
    eprintln!("Building fastgrep index...");
    let idx_start = Instant::now();
    let status = Command::new(&fg_path)
        .args(["index", "--path", corpus_path])
        .stderr(std::process::Stdio::inherit())
        .status()?;
    if !status.success() {
        anyhow::bail!("Failed to build fastgrep index");
    }
    let idx_time = idx_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Index built in {:.0}ms\n", idx_time);

    // Warmup: run each tool once to populate OS page cache
    eprintln!("Warming up...");
    let _ = Command::new(&rg_path)
        .args(["--count-matches", "fn"])
        .current_dir(corpus)
        .output();
    let _ = Command::new(&fg_path)
        .args(["search", "fn", "--path", &corpus.to_string_lossy(), "--format", "json", "--no-auto-index"])
        .output();
    eprintln!();

    let mut results = Vec::new();

    for &(name, pattern) in PATTERNS {
        eprint!("  {:<20}", name);

        let mut rg_times = Vec::new();
        let mut fg_times = Vec::new();
        let mut rg_count = 0;
        let mut fg_count = 0;

        for i in 0..iterations {
            // Run ripgrep
            let (rg_ms, rc) = bench_rg(&rg_path, corpus, pattern)?;
            rg_count = rc;
            rg_times.push(rg_ms);
            results.push(BenchResult {
                pattern_name: name.to_string(),
                pattern: pattern.to_string(),
                tool: "rg".to_string(),
                iteration: i,
                wall_time_ms: rg_ms,
                match_count: rc,
            });

            // Run fastgrep
            let (fg_ms, fc) = bench_fastgrep(&fg_path, corpus, pattern)?;
            fg_count = fc;
            fg_times.push(fg_ms);
            results.push(BenchResult {
                pattern_name: name.to_string(),
                pattern: pattern.to_string(),
                tool: "fastgrep".to_string(),
                iteration: i,
                wall_time_ms: fg_ms,
                match_count: fc,
            });
        }

        let rg_median = median(&mut rg_times);
        let fg_median = median(&mut fg_times);
        let speedup = if fg_median > 0.0 { rg_median / fg_median } else { 0.0 };
        let match_ok = if rg_count == fg_count { "✓" } else { "✗ MISMATCH" };

        eprintln!(
            "rg={:>7.1}ms  fg={:>7.1}ms  {:>5.1}x  matches: rg={} fg={} {}",
            rg_median, fg_median, speedup, rg_count, fg_count, match_ok,
        );
    }

    // Write CSV
    let mut wtr = std::fs::File::create(output)?;
    use std::io::Write;
    writeln!(wtr, "pattern_name,pattern,tool,iteration,wall_time_ms,match_count")?;
    for r in &results {
        writeln!(
            wtr,
            "{},{},{},{},{:.2},{}",
            r.pattern_name,
            r.pattern.replace(',', "\\,"),
            r.tool,
            r.iteration,
            r.wall_time_ms,
            r.match_count,
        )?;
    }

    eprintln!("\nResults written to {}", output);
    Ok(())
}

fn bench_rg(rg_path: &Path, corpus: &Path, pattern: &str) -> Result<(f64, usize)> {
    let start = Instant::now();
    let output = Command::new(rg_path)
        .args(["-c", pattern])  // -c counts matching lines (not match occurrences)
        .current_dir(corpus)
        .output()?;
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let count: usize = stdout
        .lines()
        .filter_map(|l| l.rsplit(':').next()?.parse::<usize>().ok())
        .sum();

    Ok((elapsed, count))
}

fn bench_fastgrep(fg_path: &Path, corpus: &Path, pattern: &str) -> Result<(f64, usize)> {
    let start = Instant::now();
    let output = Command::new(fg_path)
        .args([
            "search", pattern,
            "--path", &corpus.to_string_lossy(),
            "--format", "json",
            "--no-auto-index",
        ])
        .output()?;
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let count = stdout.lines().filter(|l| !l.is_empty()).count();

    Ok((elapsed, count))
}

fn median(times: &mut Vec<f64>) -> f64 {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if times.is_empty() {
        return 0.0;
    }
    if times.len() % 2 == 0 {
        (times[times.len() / 2 - 1] + times[times.len() / 2]) / 2.0
    } else {
        times[times.len() / 2]
    }
}
