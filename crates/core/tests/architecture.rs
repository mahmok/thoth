//! Guards the domain ring's dependency rule.
//!
//! `domain` may import only:
//!
//! - `crate::domain`, so the ring may use its own records and rules;
//! - the standard library (`std`, `core`, `alloc`), except the filesystem, locks and the system
//!   clock;
//! - `serde` (the stored `Provider` shape of ADR-0001) and `serde_json` (the protocol values it
//!   stores and the JSON schemas the domain rules work on);
//! - `ulid`, the identity type of [`SessionId`](crate::domain::shared::SessionId), which is a
//!   value type with no I/O of its own.
//!
//! Any other `crate::` path and every external crate this crate depends on fails the test. The
//! rule is an allowlist: a new import fails until someone decides where it belongs, rather than
//! passing because nobody thought to add it to a blocklist. The external crates are read from
//! `Cargo.toml`, so a dependency added today is covered without editing this file.

use std::fs;
use std::path::{Path, PathBuf};

/// The roots a domain source may import from, besides its own crate.
const ALLOWED_ROOTS: &[&str] = &["alloc", "core", "crate", "self", "serde", "serde_json", "std", "super", "ulid"];

/// Standard-library paths a domain source may never import: I/O, locks and the wall clock.
const FORBIDDEN_STD: &[&str] = &["std::fs", "std::sync", "std::time::SystemTime"];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn domain_dir() -> PathBuf {
    manifest_dir().join("src").join("domain")
}

/// The names of the crates this crate depends on that the domain may not import.
fn external_crates() -> Vec<String> {
    let manifest = fs::read_to_string(manifest_dir().join("Cargo.toml")).expect("Cargo.toml should be readable");
    let mut in_dependencies = false;
    let mut crates = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dependencies = trimmed.ends_with("dependencies]");
            continue;
        }
        if !in_dependencies || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_matches('"');
        // `tokio.workspace = true` and `tokio-util = ...`: the crate root is what follows.
        let name = name.split('.').next().unwrap_or(name).replace('-', "_");
        if !ALLOWED_ROOTS.contains(&name.as_str()) {
            crates.push(name);
        }
    }
    crates
}

/// One path found in a source, such as `crate::adapters::llm::client`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Import {
    line: usize,
    path: String,
}

/// Every non-comment `path::chain` in `source`, in source order.
///
/// The scanner is deliberately small: it skips line comments so that prose may mention a path,
/// and it reads identifier chains so that `Message::user` is a chain but a string is not.
fn imports(source: &str) -> Vec<Import> {
    let chars: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut line: usize = 1;
    let mut i = 0;
    while i < chars.len() {
        let current = chars[i];
        if current == '\n' {
            line = line.saturating_add(1);
            i = i.saturating_add(1);
            continue;
        }
        if current == '/' && chars.get(i.saturating_add(1)) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i = i.saturating_add(1);
            }
            continue;
        }
        if !is_identifier_start(current) {
            i = i.saturating_add(1);
            continue;
        }

        let start = line;
        let mut segments: Vec<String> = Vec::new();
        loop {
            let mut segment = String::new();
            while i < chars.len() && is_identifier_continue(chars[i]) {
                segment.push(chars[i]);
                i = i.saturating_add(1);
            }
            segments.push(segment);

            let mut through = i;
            while chars.get(through) == Some(&' ') {
                through = through.saturating_add(1);
            }
            let is_path = chars.get(through) == Some(&':') && chars.get(through.saturating_add(1)) == Some(&':');
            if !is_path {
                break;
            }
            i = through.saturating_add(2);
            while chars.get(i) == Some(&' ') {
                i = i.saturating_add(1);
            }
            if !chars.get(i).is_some_and(|character| is_identifier_start(*character)) {
                break;
            }
        }

        if segments.len() >= 2 {
            found.push(Import { line: start, path: segments.join("::") });
        }
    }
    found
}

const fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

const fn is_identifier_continue(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// Why `path` may not appear in a domain source, or `None` when it may.
fn violation(path: &str, external: &[String]) -> Option<String> {
    let mut segments = path.split("::");
    let root = segments.next()?;
    // A chain that starts in upper case is a type path such as `Message::user`, not a module.
    if root.starts_with(|character: char| character.is_ascii_uppercase()) {
        return None;
    }
    if root == "crate" {
        let second = segments.next().unwrap_or_default();
        if second != "domain" {
            return Some(format!("`{path}`: `crate::` imports must stay inside `crate::domain`"));
        }
        return None;
    }
    if root == "std" {
        let forbidden = FORBIDDEN_STD.iter().copied().find(|forbidden| {
            path == *forbidden || path.strip_prefix(forbidden).is_some_and(|rest| rest.starts_with("::"))
        });
        if let Some(forbidden) = forbidden {
            return Some(format!("`{path}`: `{forbidden}` is infrastructure, not domain"));
        }
        return None;
    }
    if external.iter().any(|name| name == root) {
        return Some(format!("`{path}`: the crate `{root}` is outside the domain ring"));
    }
    // Anything else is a local module or an imported alias (`tool_call`, `fmt`, `numbers`), or a
    // primitive type path (`str::to_owned`), all of which resolve inside the crate.
    None
}

/// Every rule violation in `source`, in declaration order.
fn violations(source: &str, external: &[String]) -> Vec<String> {
    imports(source)
        .into_iter()
        .filter_map(|import| violation(&import.path, external).map(|reason| format!("line {}: {reason}", import.line)))
        .collect()
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
fn the_scanner_finds_each_kind_of_path() {
    let found = imports(
        "use crate::adapters::llm::client::chat::Message;\n\
         let now = crate::clock::now_ms();\n\
         Message::user(\"hi\");\n\
         // a comment about crate::runtime::agent\n",
    );
    let paths: Vec<&str> = found.iter().map(|import| import.path.as_str()).collect();
    assert_eq!(
        paths,
        ["crate::adapters::llm::client::chat::Message", "crate::clock::now_ms", "Message::user"],
        "the scanner should read paths and ignore comments"
    );
    assert_eq!(found[0].line, 1);
    assert_eq!(found[1].line, 2);
}

#[test]
fn the_external_crates_come_from_the_manifest() {
    let external = external_crates();
    assert!(external.iter().any(|name| name == "tokio"), "tokio is a dependency and must be covered: {external:?}");
    assert!(external.iter().any(|name| name == "reqwest"), "reqwest is a dependency: {external:?}");
    for allowed in ["serde", "serde_json"] {
        assert!(!external.iter().any(|name| name == allowed), "{allowed} is the ADR-0001 exception");
    }
}

#[test]
fn detects_each_forbidden_import() {
    let forbidden: &[(&str, &str)] = &[
        ("use crate::adapters::llm::client::chat::Message;", "crate::adapters"),
        ("use crate::runtime::types::AgentMessage;", "crate::runtime"),
        ("use crate::clock::now_ms;", "crate::clock"),
        ("use crate::application::ports::Reply;", "crate::application"),
        ("use tokio::sync::Mutex;", "tokio"),
        ("use reqwest::Client;", "reqwest"),
        ("use sqlx::SqlitePool;", "sqlx"),
        ("use std::fs::File;", "std::fs"),
        ("use std::sync::Mutex;", "std::sync"),
        ("let now = std::time::SystemTime::now();", "std::time::SystemTime"),
    ];
    let external = external_crates();
    for &(source, needle) in forbidden {
        let reasons = violations(source, &external);
        assert!(
            reasons.iter().any(|reason| reason.contains(needle)),
            "the guard should flag `{source}`, got {reasons:?}"
        );
    }
}

#[test]
fn allowed_imports_are_not_flagged() {
    let source = "\
use serde::{Deserialize, Serialize};\n\
use serde_json::Value;\n\
use std::collections::HashMap;\n\
use std::time::Duration;\n\
use core::fmt;\n\
use alloc::sync::Arc;\n\
use crate::domain::conversation::Conversation;\n\
use tool_call::ToolCall;\n\
let message = Message::user(\"hi\");\n\
self.value(); super::helper();\n";
    assert_eq!(
        violations(source, &external_crates()),
        Vec::<String>::new(),
        "the domain may use serde and the prelude"
    );
}

#[test]
fn domain_sources_import_nothing_from_an_outer_ring() {
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

    let external = external_crates();
    let failures: Vec<String> = sources
        .iter()
        .flat_map(|source| {
            let text = fs::read_to_string(source).expect("domain source should be readable");
            violations(&text, &external).into_iter().map(move |reason| format!("{}: {reason}", source.display()))
        })
        .collect();

    assert!(
        failures.is_empty(),
        "domain sources must import only crate::domain, the standard library and serde:\n{}",
        failures.join("\n")
    );
}
