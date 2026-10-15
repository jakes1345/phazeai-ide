//! `git blame --porcelain` parsing for the inline blame annotation.

use std::collections::HashMap;

/// Convert days since 1970-01-01 to a (year, month, day) civil date.
/// (Howard Hinnant's `civil_from_days`; exact for the full proleptic Gregorian range.)
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse porcelain output into one `(author, YYYY-MM-DD)` entry per source line.
///
/// Porcelain prints a commit's author details only the first time that commit appears;
/// later lines from the same commit carry just the header, so details are keyed by SHA.
pub fn parse_blame_porcelain(text: &str) -> Vec<(String, String)> {
    let mut commits: HashMap<String, (String, String)> = HashMap::new();
    let mut current = String::new();
    let mut out = Vec::new();

    for line in text.lines() {
        if line.starts_with('\t') {
            out.push(commits.get(&current).cloned().unwrap_or_default());
            continue;
        }
        let first = line.split(' ').next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            current = first.to_string();
            commits.entry(current.clone()).or_default();
        } else if let Some(author) = line.strip_prefix("author ") {
            commits.entry(current.clone()).or_default().0 = author.to_string();
        } else if let Some(ts) = line.strip_prefix("author-time ") {
            if let Ok(secs) = ts.trim().parse::<i64>() {
                let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
                commits.entry(current.clone()).or_default().1 = format!("{y:04}-{m:02}-{d:02}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_exact() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29)); // leap day
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn interleaved_commits_keep_their_own_author() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        // Commit A appears on lines 1 and 3, commit B on line 2.
        // Details for A are only printed the first time it appears.
        let text = format!(
            "{a} 1 1 1\nauthor Alice\nauthor-mail <a@x>\nauthor-time 1704067200\nsummary one\n\tline one\n\
             {b} 2 2 1\nauthor Bob\nauthor-mail <b@x>\nauthor-time 1709251200\nsummary two\n\tline two\n\
             {a} 3 3 1\n\tline three\n"
        );
        let entries = parse_blame_porcelain(&text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("Alice".into(), "2024-01-01".into()));
        assert_eq!(entries[1], ("Bob".into(), "2024-03-01".into()));
        assert_eq!(
            entries[2],
            ("Alice".into(), "2024-01-01".into()),
            "line 3 must not inherit Bob"
        );
    }

    #[test]
    fn empty_or_garbage_output_yields_no_entries() {
        assert!(parse_blame_porcelain("").is_empty());
        assert!(parse_blame_porcelain("fatal: no such path").is_empty());
    }
}
