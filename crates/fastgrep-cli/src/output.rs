/// Output formatting for search results.

use fastgrep_core::query::execute::SearchMatch;

/// Print matches in ripgrep-compatible text format.
pub fn print_text(matches: &[SearchMatch]) {
    let use_color = supports_color();
    let mut last_file = "";
    for m in matches {
        if m.file != last_file {
            if use_color {
                println!("\x1b[35m{}\x1b[0m", m.file);
            } else {
                println!("{}", m.file);
            }
            last_file = &m.file;
        }
        if use_color {
            println!("\x1b[32m{}\x1b[0m:{}", m.line_number, m.line);
        } else {
            println!("{}:{}", m.line_number, m.line);
        }
    }
}

/// Print matches as JSON Lines.
pub fn print_json(matches: &[SearchMatch]) -> anyhow::Result<()> {
    for m in matches {
        let json = serde_json::to_string(m)?;
        println!("{}", json);
    }
    Ok(())
}

/// Check if we should use ANSI colors.
fn supports_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}
