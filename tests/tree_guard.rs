//! A denylist of tokens that must not appear anywhere in this repository.
//!
//! The corpus this module was first exercised against, and the project it belongs to,
//! are private. The words that would identify either are held here as SHA-256 hashes
//! of lowercase word tokens rather than as text, so the guard can scan itself and
//! still pass. That hides the names from a READER; it hides nothing from anyone who
//! hashes a guess, and that is the intent — this is a regression guard against a word
//! creeping back in, not a secret.
//!
//! Every tracked (and every not-yet-ignored, not-yet-tracked) file is tokenized on
//! anything that is not a letter or a digit, so a URL or a path is scanned by its
//! parts. One of the denylisted tokens is an ordinary English word, so an innocent use
//! trips this too: rephrase rather than relax the guard.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// SHA-256 of each denylisted token, lowercase, hex.
const DENIED: [&str; 4] = [
    "d5521e3b22d354c93e29236d6e29e8de369ae95e5f69b616b54b25b440c9f8ff",
    "80c70472eed5bf6238cf7eae9804f2eb1c879b7ddba4adc7f4943d55e9a4dd56",
    "a9f6ff6e89fa31b479b7ab8608323211a49d08e1be47705556eca97b0c7860bd",
    "4d3f1365b7bbeb4a23893f33e86b66255ad097119ba66aacbda72d992cf42ac7",
];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every file to scan: what git tracks, plus anything new that is not ignored.
fn files() -> Vec<PathBuf> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(repo())
        .output()
        .expect("run git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let listed: Vec<PathBuf> = String::from_utf8(out.stdout)
        .expect("git ls-files output is UTF-8")
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| repo().join(s))
        .collect();
    assert!(
        listed.len() > 10,
        "expected a repository's worth of files, got {}",
        listed.len()
    );
    listed
}

fn denied(token: &str) -> bool {
    let digest = format!("{:x}", Sha256::digest(token.as_bytes()));
    DENIED.contains(&digest.as_str())
}

/// The tokens of one line: lowercase runs of letters and digits.
fn tokens(line: &str) -> impl Iterator<Item = String> + '_ {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
}

fn scan(path: &Path, hits: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return; // binary or unreadable: nothing to read words in
    };
    for (n, line) in text.lines().enumerate() {
        if tokens(line).any(|t| denied(&t)) {
            hits.push(format!("{}:{}", path.display(), n + 1));
        }
    }
    // A path can carry a word the file's contents do not.
    if tokens(&path.to_string_lossy()).any(|t| denied(&t)) {
        hits.push(format!("{}: (in the path)", path.display()));
    }
}

#[test]
fn no_tracked_file_contains_a_denylisted_token() {
    let mut hits = Vec::new();
    for path in files() {
        scan(&path, &mut hits);
    }
    assert!(
        hits.is_empty(),
        "a denylisted token appears here; rephrase:\n{}",
        hits.join("\n")
    );
}

/// The mechanism itself — tokenizing and hash-matching — over a word this test picks,
/// so the check is exercised without writing any denylisted word down.
#[test]
fn the_guard_matches_whole_tokens_by_hash() {
    let digest = |t: &str| format!("{:x}", Sha256::digest(t.as_bytes()));
    let wanted = digest("sampletoken");
    let matches = |line: &str| tokens(line).any(|t| digest(&t) == wanted);

    // Case is folded, and punctuation, path separators and quoting are no hiding
    // place: a URL or a path is scanned by its parts.
    assert!(matches("a line with SampleToken in it"));
    assert!(matches("https://example.org/sampleToken/x"));
    assert!(matches("`sampletoken`,"));
    // The flip side of splitting on punctuation: a word broken ACROSS a separator is
    // two tokens and does not match, and a longer word containing it is a different
    // token. Neither is caught — this guard is against a word coming back, not
    // against someone hiding one.
    assert!(!matches("sample-token"));
    assert!(!matches("sampletokens are fine"));

    // And the real list is the one the scan uses.
    assert_eq!(DENIED.len(), 4);
    assert!(!denied("sampletoken"));
}
