//! Source-level guard for a call that compiles fine and must never be made.
//!
//! `otto_tenant::Db::migrate` is a perfectly reasonable method that is wrong
//! *for this deployment*: it would apply otto-platform's own migration history
//! to this database, and the only thing between a caller and the mistake is that
//! nobody reaches for it. A test that reads the source is crude but it is the one
//! thing that fails when someone does. Go through `of_core::migrate`.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name != "target" && !name.starts_with('.') {
                rust_sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file in the workspace's `crates/` directory, with its text.
fn workspace_sources() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_sources(&crates, &mut files);
    files
        .into_iter()
        .map(|p| {
            let text = fs::read_to_string(&p).expect("read source");
            (p, text)
        })
        .collect()
}

/// Lines of `text` that are code, not comments.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
}

#[test]
fn nothing_runs_the_platforms_migrations_on_this_database() {
    let this_file = Path::new(file!()).file_name().unwrap().to_owned();
    let mut offenders = Vec::new();

    for (path, text) in workspace_sources() {
        if path.file_name() == Some(&this_file) {
            continue;
        }
        for (n, line) in code_lines(&text) {
            if line.contains(".migrate()") || line.contains("otto_tenant::db::MIGRATOR") {
                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "this database has its own migration history (`of_core::migrate`, \
         `of_core::MIGRATOR`); otto-platform's would collide with it:\n{}",
        offenders.join("\n")
    );
}
