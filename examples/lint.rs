//! The mapping linter from the command line: the three checks, and a snapshot to compare
//! with later.
//!
//! ```text
//! cargo run --example lint -- mapping  <mapping>
//! cargo run --example lint -- expects  <mapping>
//! cargo run --example lint -- snapshot <corpus-root> <mapping> [subdir…]  > baseline.nq
//! cargo run --example lint -- corpus   <corpus-root> <mapping> [--baseline FILE]
//!                                      [--ignore IRI]… [subdir…]
//! ```
//!
//! Add `--turtle` to any check for the Turtle face. `<mapping>` is a NAME (a mapping
//! installed in the config home, `urn:markdown:mapping:{name}`) or a PATH to a
//! `mapping.ttl`. Every `*.md` under each `subdir` (default: the whole root) is a document,
//! bound as `urn:corpus:{path}` with the root `urn:corpus:`, so a document's graph is
//! `urn:markdown:doc:{path}` and a snapshot taken on one machine compares with a corpus on
//! another. Nothing is written but standard output.
//!
//! To compare with an earlier revision of a corpus kept in git, take the snapshot from a
//! copy of that revision (`git archive <rev> | tar -x -C <dir>`), then run `corpus` on the
//! working tree with `--baseline`.
//!
//! The exit status is 1 when a text report holds an error, 2 on a usage or input problem.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};
use ikigai_markdown::MappingHome;

const MAPPING_IRI: &str = "urn:corpus-mapping:file";
const BASELINE_IRI: &str = "urn:corpus-baseline:file";
const ROOT: &str = "urn:corpus:";

fn bound(id: &str, media: &'static str, bytes: Vec<u8>) -> FnEndpoint {
    FnEndpoint::new(id.to_string(), move |_| {
        Ok(Representation::new(ReprType::new(media), bytes.clone()).cacheable())
    })
    .with_description(
        Description::new(id)
            .summary("a file bound for the linter")
            .verb(Verb::Source),
    )
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let hidden = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Percent-encode a relative path for an IRI, keeping `/`.
fn escape(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn die(message: &str) -> ! {
    eprintln!("lint: {message}");
    std::process::exit(2);
}

/// The IRI to pass as `mapping=`, binding the file into `space` when it is a path.
fn mapping(arg: &str, space: EndpointSpace) -> (String, EndpointSpace) {
    let as_path = Path::new(arg);
    if arg.contains(['/', '\\']) || arg.ends_with(".ttl") || as_path.exists() {
        let bytes = std::fs::read(as_path).unwrap_or_else(|e| die(&format!("{arg}: {e}")));
        let space = space.bind(
            Exact::new(MAPPING_IRI),
            bound("mapping", "text/turtle", bytes),
        );
        return (MAPPING_IRI.to_string(), space);
    }
    (format!("urn:markdown:mapping:{arg}"), space)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let turtle = take_flag(&mut args, "--turtle");
    let baseline = take_value(&mut args, "--baseline");
    let mut ignore = Vec::new();
    while let Some(i) = take_value(&mut args, "--ignore") {
        ignore.push(i);
    }
    let usage = "usage: lint mapping|expects <mapping> | lint snapshot|corpus <corpus-root> \
                 <mapping> [--baseline FILE] [--ignore IRI]… [--turtle] [subdir…]";
    let Some(command) = args.first().cloned() else {
        die(usage)
    };

    let space = ikigai_markdown::space_with(Arc::new(MappingHome::ambient()));
    let mut request = match command.as_str() {
        "mapping" | "expects" => {
            let [_, m] = args.as_slice() else { die(usage) };
            let (iri, space) = mapping(m, space);
            let target = if command == "mapping" {
                ikigai_markdown::LINT_MAPPING_IRI
            } else {
                ikigai_markdown::LINT_EXPECTS_IRI
            };
            let request = Request::new(Verb::Source, Iri::parse(target).unwrap())
                .with_arg("mapping", ArgRef::Inline(iri.into_bytes()));
            (request, space)
        }
        "snapshot" | "corpus" => {
            if args.len() < 3 {
                die(usage);
            }
            let root = PathBuf::from(&args[1]);
            let (iri, mut space) = mapping(&args[2], space);
            let dirs: Vec<PathBuf> = if args.len() > 3 {
                args[3..].iter().map(|s| root.join(s)).collect()
            } else {
                vec![root.clone()]
            };
            let mut files = Vec::new();
            for dir in &dirs {
                walk(dir, &mut files);
            }
            let mut docs = Vec::new();
            for (i, file) in files.iter().enumerate() {
                let rel = file
                    .strip_prefix(&root)
                    .expect("under root")
                    .to_string_lossy()
                    .to_string();
                let doc = format!("{ROOT}{}", escape(&rel));
                let bytes = std::fs::read(file)
                    .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
                space = space.bind(
                    Exact::new(doc.as_str()),
                    bound(&format!("doc{i}"), "text/markdown", bytes),
                );
                docs.push(doc);
            }
            let target = if command == "snapshot" {
                ikigai_markdown::SNAPSHOT_IRI
            } else {
                ikigai_markdown::LINT_CORPUS_IRI
            };
            let mut request = Request::new(Verb::Source, Iri::parse(target).unwrap())
                .with_arg("docs", ArgRef::Inline(docs.join("\n").into_bytes()))
                .with_arg("root", ArgRef::Inline(ROOT.as_bytes().to_vec()))
                .with_arg("mapping", ArgRef::Inline(iri.into_bytes()));
            if let Some(b) = &baseline {
                let bytes = std::fs::read(b).unwrap_or_else(|e| die(&format!("{b}: {e}")));
                space = space.bind(
                    Exact::new(BASELINE_IRI),
                    bound("baseline", "application/n-quads", bytes),
                );
                request = request.with_arg("baseline", ArgRef::Inline(BASELINE_IRI.into()));
            }
            if !ignore.is_empty() {
                request = request.with_arg("ignore", ArgRef::Inline(ignore.join(" ").into_bytes()));
            }
            (request, space)
        }
        _ => die(usage),
    };
    if turtle && command != "snapshot" {
        request.0 = request
            .0
            .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    }
    let (request, space) = request;
    let kernel = Kernel::new(Arc::new(space));
    // Root: this is the operator's own terminal, reading the operator's own files, and
    // root's time budget is the ceiling, which a whole corpus may need.
    let answer = futures::executor::block_on(kernel.issue(request, &Capability::root()))
        .unwrap_or_else(|e| die(&e.to_string()));
    let text = String::from_utf8_lossy(&answer.bytes);
    print!("{text}");
    let failed = !turtle
        && command != "snapshot"
        && command != "expects"
        && text
            .lines()
            .find_map(|l| l.strip_prefix("  result: "))
            .is_some_and(|r| !r.starts_with("0 errors"));
    if failed {
        std::process::exit(1);
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    match args.iter().position(|a| a == flag) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    if i + 1 >= args.len() {
        die(&format!("{flag} needs a value"));
    }
    let value = args.remove(i + 1);
    args.remove(i);
    Some(value)
}
