//! Format a file's text with its language's formatter (rustfmt, prettier,
//! black), for format-on-save.
//!
//! The text goes through the formatter's stdin and comes back on stdout, so
//! the formatter writes nothing itself: run on a path, rustfmt also rewrote
//! every module the file declares (`mod util;`), and it turned CRLF line
//! endings into LF. Here only the text given changes, and a CRLF file stays
//! CRLF.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// How long a formatter may run before it is stopped and the text is left
/// as it was.
const TIME_LIMIT: Duration = Duration::from_secs(10);

/// The Rust edition of the crate holding `file`: the nearest `Cargo.toml`
/// above it that names one (`edition.workspace = true` defers to one
/// further up). 2021 when none does; rustfmt alone would assume 2015.
pub fn rust_edition(file: &Path) -> String {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Ok(manifest) = std::fs::read_to_string(d.join("Cargo.toml")) {
            for line in manifest.lines() {
                let line = line.trim();
                let Some(rest) = line.strip_prefix("edition") else {
                    continue;
                };
                let rest = rest.trim_start();
                if let Some(value) = rest.strip_prefix('=') {
                    let value = value.trim().trim_matches('"');
                    if value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()) {
                        return value.to_string();
                    }
                }
            }
        }
        dir = d.parent();
    }
    "2021".to_string()
}

/// The formatter for `path` (by extension), as a program and its arguments
/// for reading the text on stdin.
fn formatter_for(path: &Path) -> Option<(&'static str, Vec<String>)> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    let shown = path.to_string_lossy().to_string();
    match ext {
        "rs" => Some((
            "rustfmt",
            vec![
                "--edition".to_string(),
                rust_edition(path),
                "--emit".to_string(),
                "stdout".to_string(),
            ],
        )),
        "js" | "ts" | "jsx" | "tsx" | "json" => {
            Some(("prettier", vec!["--stdin-filepath".to_string(), shown]))
        }
        "py" => Some((
            "black",
            vec![
                "-q".to_string(),
                "--stdin-filename".to_string(),
                shown,
                "-".to_string(),
            ],
        )),
        _ => None,
    }
}

/// `text` (the contents of `path`) as its formatter writes it. `None` when
/// there is no formatter for the file, it is not installed, it fails (a
/// syntax error, say) or it runs past [`TIME_LIMIT`]. The formatter runs in
/// the file's folder, so it finds the project's own config (`rustfmt.toml`,
/// `.prettierrc`, `pyproject.toml`). Line endings are the text's own.
pub fn format_text(path: &Path, text: &str) -> Option<String> {
    let (program, args) = formatter_for(path)?;
    let dir: PathBuf = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut child = phazeai_core::process::std_command(program)
        .args(&args)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok().map(|_| out)
    });
    let deadline = Instant::now() + TIME_LIMIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let _ = writer.join();
    let out = reader.join().ok()??;
    if !status.success() || out.is_empty() && !text.is_empty() {
        return None;
    }
    Some(with_line_endings_of(text, &out))
}

/// `formatted` with CRLF line endings when `original` uses them.
fn with_line_endings_of(original: &str, formatted: &str) -> String {
    if original.contains("\r\n") {
        formatted.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        formatted.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(program: &str) -> bool {
        phazeai_core::process::std_command(program)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn the_edition_comes_from_the_nearest_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace.package]\nedition = \"2024\"\n",
        )
        .unwrap();
        let krate = dir.path().join("crates/a");
        std::fs::create_dir_all(krate.join("src")).unwrap();
        std::fs::write(
            krate.join("Cargo.toml"),
            "[package]\nname = \"a\"\nedition.workspace = true\n",
        )
        .unwrap();
        assert_eq!(rust_edition(&krate.join("src/lib.rs")), "2024");
        std::fs::write(
            krate.join("Cargo.toml"),
            "[package]\nname = \"a\"\nedition = \"2018\"\n",
        )
        .unwrap();
        assert_eq!(rust_edition(&krate.join("src/lib.rs")), "2018");
        assert_eq!(rust_edition(Path::new("/no/such/dir/x.rs")), "2021");
    }

    #[test]
    fn rustfmt_formats_only_the_text_and_keeps_crlf() {
        if !installed("rustfmt") {
            eprintln!("rustfmt not installed; nothing to check");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.rs");
        let util = dir.path().join("util.rs");
        let messy_util = "pub fn f( ){ }\n";
        std::fs::write(&util, messy_util).unwrap();
        let text = "mod util;\r\nfn main(){let x=1;}\r\n";
        std::fs::write(&main, text).unwrap();

        let out = format_text(&main, text).expect("rustfmt formats valid code");
        assert_eq!(out, "mod util;\r\nfn main() {\r\n    let x = 1;\r\n}\r\n");
        assert_eq!(
            std::fs::read_to_string(&util).unwrap(),
            messy_util,
            "a module the file declares is left alone"
        );
        assert_eq!(
            std::fs::read_to_string(&main).unwrap(),
            text,
            "the formatter writes no file itself"
        );
        // Async needs the 2018 edition or later, which rustfmt does not
        // assume on stdin; the edition from Cargo.toml is passed.
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nedition = \"2021\"\n",
        )
        .unwrap();
        assert!(format_text(&main, "async fn f(){}\n").is_some());
        assert_eq!(
            format_text(&main, "fn main({"),
            None,
            "a syntax error changes nothing"
        );
    }

    #[test]
    fn prettier_and_black_read_the_text_from_stdin() {
        let dir = tempfile::tempdir().unwrap();
        if installed("prettier") {
            let js = dir.path().join("a.js");
            assert_eq!(
                format_text(&js, "const a  =  {b:1}\r\n").as_deref(),
                Some("const a = { b: 1 };\r\n")
            );
        }
        if installed("black") {
            let py = dir.path().join("a.py");
            assert_eq!(
                format_text(&py, "x=[1,2 ,3]\n").as_deref(),
                Some("x = [1, 2, 3]\n")
            );
        }
        assert_eq!(format_text(&dir.path().join("notes.txt"), "a  b"), None);
    }
}
