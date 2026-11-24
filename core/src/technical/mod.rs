//! The technical half of phin's core: parsing files and PDFs, talking to the
//! front ends over the wire protocol, and running helper processes.
//!
//! This is the counterpart of [`core::domain`], which stays pure. Anything that
//! performs I/O belongs here; the domain only ever sees the data this side
//! produces.

pub mod csv;
pub mod fs_atomic;
pub mod id;
pub mod pdf_text;
pub mod state;
pub mod store_transactions;
pub mod store_sidefiles;
pub mod transport;
pub mod transport_cfg;
pub mod view;

#[test]
fn technical_module_compiles() {}

/// Locate `core/src/domain` relative to the test's working directory.
///
/// `cargo test` runs the test binary with the package root (`core/`) as its
/// working directory, so the plain relative path resolves on the first try;
/// the `core/` prefix covers a run started one level up, at the workspace root.
/// Only those two candidates are tried per directory, and the search walks up
/// the parents of the current directory, so it never scans anything beyond the
/// handful of files under `core/src/domain/`.
#[cfg(test)]
fn find_domain_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("src").join("domain");
        if candidate.is_dir() { return candidate; }
        let packaged = dir.join("core").join("src").join("domain");
        if packaged.is_dir() { return packaged; }
        match dir.parent() {
            None => { panic!("purity test cannot find core/src/domain relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// Offending lines under `dir`: one entry per line that names a module the
/// domain must not use.
///
/// Walks the `.rs` files below `dir` and returns `file:line:source` for every
/// line that mentions a forbidden standard-library module. The check is textual
/// on purpose: the plan asks for a grep, and a source-level mention is enough
/// to tell a reviewer that the domain has started doing I/O.
#[cfg(test)]
fn collect_offenders(dir: std::path::PathBuf) -> Vec::<String> {
    const FORBIDDEN: [&str; 2] = ["std::fs", "std::net"];
    let mut offenders = Vec::<String>::new();
    let mut pending = Vec::<std::path::PathBuf>::new();
    pending.push(dir);
    while !pending.is_empty() {
        let current = pending.pop().unwrap();
        for entry in std::fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.to_path_buf());
                continue;
            }
            let shown = path.to_string_lossy().to_string();
            if !shown.ends_with(".rs") { continue; }
            let source = std::fs::read_to_string(path).unwrap();
            for (index, line) in source.lines().enumerate() {
                for word in FORBIDDEN {
                    if line.contains(word) {
                        let trimmed = line.trim();
                        let number = index + 1;
                        offenders.push(format!("{shown}:{number}: {trimmed}"));
                    }
                }
            }
        }
    }
    return offenders;
}

/// Enforce the purity rule of the domain subtree.
///
/// Fails on the first source line under `core/src/domain/` that names a
/// forbidden standard-library module; the offending lines are printed before
/// the assertion fires.
#[test]
fn domain_sources_are_pure() {
    let dir = find_domain_dir();
    let offenders = collect_offenders(dir);
    if !offenders.is_empty() {
        for line in &offenders {
            println!("purity violation: {line}");
        }
    }
    assert!(offenders.is_empty(), "core/src/domain must stay pure: no file system, no network");
}
