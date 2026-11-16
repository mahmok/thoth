//! Guards the domain ring's dependency rule.
//!
//! `domain` may depend on `serde` (the stored `Provider` shape, ADR-0001) and the
//! standard prelude. Everything else — async runtimes, the filesystem, the clock,
//! HTTP, SQLite, auth and protocol clients — belongs to an outer ring.
//!
//! Phase 0 of #41 lands this guard before the `domain` folder exists. The
//! detector is exercised against synthetic sources today; the directory scan
//! takes over as soon as `src/domain` appears.

use std::fs;
use std::path::{Path, PathBuf};

/// Imports that may never appear in a domain source file.
const FORBIDDEN: &[&str] = &[
    "tokio::",
    "std::sync",
    "std::fs",
    "reqwest",
    "sqlx",
    "crate::auth",
    "crate::llm::client",
    "std::time::SystemTime",
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn domain_dir() -> PathBuf {
    manifest_dir().join("src").join("domain")
}

/// The forbidden imports the guard finds in `source`, in declaration order.
fn forbidden_hits(source: &str) -> Vec<&'static str> {
    FORBIDDEN.iter().copied().filter(|needle| source.contains(needle)).collect()
}

/// Every Rust source under `dir`, recursively, in a stable order.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .expect("domain directory should be readable")
        .map(|entry| entry.expect("entry").path())
        .collect();
    entries.sort();

    let mut sources = Vec::new();
    for path in entries {
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    sources
}

#[test]
fn detects_each_forbidden_import() {
    for needle in FORBIDDEN {
        let source = format!("use {needle}Something;");
        assert_eq!(forbidden_hits(&source), vec![*needle], "the guard should flag `{needle}` in a domain source");
    }
}

#[test]
fn allowed_imports_are_not_flagged() {
    let source =
        "use serde::Serialize;\nuse std::collections::HashMap;\nuse crate::domain::conversation::Conversation;\n";
    assert!(forbidden_hits(source).is_empty(), "serde and the standard prelude are allowed in domain");
}

#[test]
fn domain_sources_import_no_infrastructure() {
    let domain = domain_dir();
    let declares_domain = fs::read_to_string(manifest_dir().join("src/lib.rs"))
        .expect("lib.rs should be readable")
        .contains("mod domain;");

    if !domain.exists() {
        // The folder split (#41) has not landed. Fail loudly if the module was
        // declared but the folder is missing, so the guard cannot silently rot.
        assert!(!declares_domain, "lib.rs declares `mod domain;` but {} does not exist", domain.display());
        return;
    }

    assert!(declares_domain, "{} exists but lib.rs does not declare it", domain.display());

    let sources = rust_sources(&domain);
    assert!(!sources.is_empty(), "{} contains no Rust sources, so the guard would pass vacuously", domain.display());

    let failures: Vec<String> = sources
        .iter()
        .flat_map(|source| {
            let text = fs::read_to_string(source).expect("domain source should be readable");
            forbidden_hits(&text).into_iter().map(move |needle| format!("{} imports `{needle}`", source.display()))
        })
        .collect();

    assert!(failures.is_empty(), "domain sources must not import infrastructure:\n{}", failures.join("\n"));
}
