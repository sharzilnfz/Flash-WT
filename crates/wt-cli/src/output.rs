//! Presentation layer for `wt`: human-friendly byte, duration, and
//! count formatting plus aligned terminal table rendering. Every
//! command routes human-facing numbers through here so receipts share
//! one typography and one set of arithmetic.

use std::fmt;

/// Bytes scaled to the largest of B / KB / MB / GB. Base-1024 units
/// with one decimal place above the byte range (`1024` -> `1.0 KB`).
pub struct HumanBytes(pub u64);

impl fmt::Display for HumanBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;

        let b = self.0 as f64;
        if self.0 == 0 {
            write!(f, "0 B")
        } else if b < KB {
            write!(f, "{} B", self.0)
        } else if b < MB {
            write!(f, "{:.1} KB", b / KB)
        } else if b < GB {
            write!(f, "{:.1} MB", b / MB)
        } else {
            write!(f, "{:.1} GB", b / GB)
        }
    }
}

/// Seconds rendered as a coarse duration: `45s`, `12m`, `3h`, `2d`.
pub struct HumanDuration(pub u64);

impl fmt::Display for HumanDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        if s < 60 {
            write!(f, "{s}s")
        } else if s < 3600 {
            write!(f, "{}m", s / 60)
        } else if s < 86400 {
            write!(f, "{}h", s / 3600)
        } else {
            write!(f, "{}d", s / 86400)
        }
    }
}

/// Counts with standard comma digit grouping (`1234567` ->
/// `1,234,567`).
pub struct HumanCount(pub usize);

impl fmt::Display for HumanCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0.to_string();
        let mut grouped = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        f.write_str(&grouped.chars().rev().collect::<String>())
    }
}

/// Render a left-aligned table: each column is as wide as its widest
/// header or cell, columns are separated by two spaces, and rows are
/// joined with newlines (no trailing newline). Short lines are
/// trimmed, so no row carries trailing padding.
pub fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let ncols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(ncols) {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string(),
    );
    for row in rows {
        lines.push(
            (0..ncols)
                .map(|i| {
                    let cell = row.get(i).map(String::as_str).unwrap_or("");
                    format!("{:<width$}", cell, width = widths[i])
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string(),
        );
    }
    lines.join("\n")
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_zero_and_small() {
        assert_eq!(HumanBytes(0).to_string(), "0 B");
        assert_eq!(HumanBytes(1).to_string(), "1 B");
        assert_eq!(HumanBytes(512).to_string(), "512 B");
        assert_eq!(HumanBytes(1023).to_string(), "1023 B");
    }

    #[test]
    fn human_bytes_unit_boundaries() {
        assert_eq!(HumanBytes(1024).to_string(), "1.0 KB");
        assert_eq!(HumanBytes(1024 * 1024).to_string(), "1.0 MB");
        assert_eq!(HumanBytes(1024 * 1024 * 1024).to_string(), "1.0 GB");
        assert_eq!(HumanBytes(u64::MAX).to_string(), "17179869184.0 GB");
    }

    #[test]
    fn human_bytes_decimal_precision() {
        assert_eq!(HumanBytes(1536).to_string(), "1.5 KB");
        assert_eq!(HumanBytes(15 * 1024 * 1024).to_string(), "15.0 MB");
        assert_eq!(HumanBytes(2500 * 1024).to_string(), "2.4 MB");
        assert_eq!(HumanBytes(2 * 1024 * 1024 * 1024).to_string(), "2.0 GB");
    }

    #[test]
    fn human_duration_units() {
        assert_eq!(HumanDuration(0).to_string(), "0s");
        assert_eq!(HumanDuration(30).to_string(), "30s");
        assert_eq!(HumanDuration(59).to_string(), "59s");
        assert_eq!(HumanDuration(60).to_string(), "1m");
        assert_eq!(HumanDuration(120).to_string(), "2m");
        assert_eq!(HumanDuration(3599).to_string(), "59m");
        assert_eq!(HumanDuration(3600).to_string(), "1h");
        assert_eq!(HumanDuration(86399).to_string(), "23h");
        assert_eq!(HumanDuration(86400).to_string(), "1d");
        assert_eq!(HumanDuration(86400 * 3).to_string(), "3d");
    }

    #[test]
    fn human_count_digit_grouping() {
        assert_eq!(HumanCount(0).to_string(), "0");
        assert_eq!(HumanCount(999).to_string(), "999");
        assert_eq!(HumanCount(1_000).to_string(), "1,000");
        assert_eq!(HumanCount(12_345).to_string(), "12,345");
        assert_eq!(HumanCount(1_234_567).to_string(), "1,234,567");
    }

    #[test]
    fn format_table_aligns_columns_and_trims() {
        let rows = vec![
            vec!["*".into(), "main".into(), "/tmp/a".into()],
            vec![" ".into(), "feature/x".into(), "/tmp/bbb".into()],
        ];
        let out = format_table(&["", "BRANCH", "PATH"], &rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "   BRANCH     PATH");
        assert_eq!(lines[1], "*  main       /tmp/a");
        assert_eq!(lines[2], "   feature/x  /tmp/bbb");
    }
}
