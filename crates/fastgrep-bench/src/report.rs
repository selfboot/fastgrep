/// Report generation: read CSV results and produce markdown tables.

use std::collections::BTreeMap;

use anyhow::Result;

pub fn generate(input: &str) -> Result<()> {
    let content = std::fs::read_to_string(input)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.len() < 2 {
        anyhow::bail!("CSV file is empty or has no data rows");
    }

    // Parse CSV (skip header)
    // pattern_name,pattern,tool,iteration,wall_time_ms,match_count
    struct Row {
        pattern_name: String,
        tool: String,
        wall_time_ms: f64,
        match_count: usize,
    }

    let mut rows = Vec::new();
    for line in &lines[1..] {
        let parts: Vec<&str> = line.splitn(6, ',').collect();
        if parts.len() < 6 {
            continue;
        }
        rows.push(Row {
            pattern_name: parts[0].to_string(),
            tool: parts[2].to_string(),
            wall_time_ms: parts[4].parse().unwrap_or(0.0),
            match_count: parts[5].parse().unwrap_or(0),
        });
    }

    // Group by pattern_name and tool, compute median
    let mut grouped: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    let mut match_counts: BTreeMap<(String, String), usize> = BTreeMap::new();

    for row in &rows {
        let key = (row.pattern_name.clone(), row.tool.clone());
        grouped.entry(key.clone()).or_default().push(row.wall_time_ms);
        match_counts.insert(key, row.match_count);
    }

    // Compute medians
    let mut medians: BTreeMap<(String, String), f64> = BTreeMap::new();
    for (key, mut times) in grouped {
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = if times.len() % 2 == 0 {
            (times[times.len() / 2 - 1] + times[times.len() / 2]) / 2.0
        } else {
            times[times.len() / 2]
        };
        medians.insert(key, median);
    }

    // Collect unique pattern names
    let mut patterns: Vec<String> = medians
        .keys()
        .map(|(p, _)| p.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    patterns.sort();

    // Print markdown table
    println!("# Benchmark Results\n");
    println!("| Pattern | rg (ms) | fastgrep (ms) | Speedup | Matches |");
    println!("|---------|---------|---------------|---------|---------|");

    for pattern in &patterns {
        let rg_key = (pattern.clone(), "rg".to_string());
        let fg_key = (pattern.clone(), "fastgrep".to_string());

        let rg_ms = medians.get(&rg_key).copied().unwrap_or(0.0);
        let fg_ms = medians.get(&fg_key).copied().unwrap_or(0.0);
        let speedup = if fg_ms > 0.0 { rg_ms / fg_ms } else { 0.0 };
        let matches = match_counts.get(&rg_key).copied().unwrap_or(0);

        println!(
            "| {} | {:.1} | {:.1} | {:.1}x | {} |",
            pattern, rg_ms, fg_ms, speedup, matches,
        );
    }

    Ok(())
}
