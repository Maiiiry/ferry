//! Human and JSON output. Data goes to stdout, progress and diagnostics to
//! stderr; colour only when the stream is a terminal (anstream honours
//! `NO_COLOR` and `CLICOLOR_FORCE`).

use std::io::Write;

use anstyle::{AnsiColor, Style};
use serde::Serialize;

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const GREEN: Style = AnsiColor::Green.on_default();
pub const YELLOW: Style = AnsiColor::Yellow.on_default();
pub const RED: Style = AnsiColor::Red.on_default().bold();
pub const CYAN: Style = AnsiColor::Cyan.on_default();

pub fn json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let mut out = anstream::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

pub fn success(message: &str) {
    let _ = writeln!(anstream::stderr(), "{GREEN}✓{GREEN:#} {message}");
}

pub fn warning(message: &str) {
    let _ = writeln!(anstream::stderr(), "{YELLOW}warning:{YELLOW:#} {message}");
}

pub fn error(message: &str, causes: &[String], hint: Option<&str>) {
    let mut err = anstream::stderr().lock();
    let _ = writeln!(err, "{RED}error:{RED:#} {message}");
    for cause in causes {
        let _ = writeln!(err, "  {DIM}caused by:{DIM:#} {cause}");
    }
    if let Some(hint) = hint {
        let _ = writeln!(err, "  {CYAN}hint:{CYAN:#} {hint}");
    }
}

/// Prints `label  value` pairs with aligned values.
pub fn fields(rows: &[(&str, String)]) {
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut out = anstream::stdout().lock();
    for (k, v) in rows {
        let _ = writeln!(out, "  {DIM}{k:<width$}{DIM:#}  {v}");
    }
}

/// Prints a left-aligned table with a dimmed header row.
pub fn table(header: &[&str], rows: &[Vec<String>]) {
    // Cells may carry ANSI styling, which takes no columns on screen.
    let visible = |cell: &str| {
        anstream::adapter::strip_str(cell)
            .to_string()
            .chars()
            .count()
    };
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(visible(cell));
        }
    }
    let line = |cells: &[String]| {
        let last = cells.len().saturating_sub(1);
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            out.push_str(cell);
            if i != last {
                out.push_str(&" ".repeat(widths[i] - visible(cell) + 2));
            }
        }
        out
    };
    let mut out = anstream::stdout().lock();
    let head: Vec<String> = header.iter().map(|h| (*h).to_owned()).collect();
    let _ = writeln!(out, "  {DIM}{}{DIM:#}", line(&head));
    for row in rows {
        let _ = writeln!(out, "  {}", line(row));
    }
}

pub fn heading(text: &str) {
    let _ = writeln!(anstream::stdout(), "{BOLD}{text}{BOLD:#}");
}

/// "3m ago"-style age of a unix timestamp.
pub fn age(unix: u64) -> String {
    let secs = crate::fsutil::now_unix().saturating_sub(unix);
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
