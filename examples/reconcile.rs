//! Lift a corpus of markdown through the kernel, load every graph into an
//! in-memory store, and run a directory of SPARQL SELECT queries over the union.
//!
//! ```text
//! cargo run --release --example reconcile -- <corpus-root> <mapping> [subdir…]
//! ```
//!
//! `<mapping>` is either a **name** — a mapping installed in the config home, resolved
//! as `urn:markdown:mapping:{name}`, with its queries in that mapping's `queries/`
//! directory — or a **path** to a `mapping.ttl`, whose queries are then read from a
//! `queries/` directory beside it.
//!
//! Every `*.md` under each `subdir` (default: the whole root) is lifted, skipping any
//! path with an `evidence` component. Each file is bound as a kernel resource and
//! lifted BY REFERENCE with `path=` its path relative to the root — the real call a
//! host makes. Nothing is written anywhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};
use ikigai_markdown::MappingHome;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::Term;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;

const MAPPING_IRI: &str = "urn:corpus:mapping";

fn bound(id: &str, media: &'static str, bytes: Vec<u8>) -> FnEndpoint {
    FnEndpoint::new(id.to_string(), move |_| {
        Ok(Representation::new(ReprType::new(media), bytes.clone()))
    })
    .with_description(
        Description::new(id)
            .summary("a corpus file bound for the example")
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
        if path.file_name().is_some_and(|n| n == "evidence") {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

fn show(term: Option<&Term>) -> String {
    match term {
        None => String::new(),
        Some(Term::Literal(l)) => l.value().to_string(),
        Some(Term::NamedNode(n)) => n.as_str().to_string(),
        Some(other) => other.to_string(),
    }
}

fn select(store: &Store, query: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut evaluator = SparqlEvaluator::new()
        .parse_query(query)
        .map_err(|e| e.to_string())?;
    evaluator.dataset_mut().set_default_graph_as_union();
    let QueryResults::Solutions(solutions) = evaluator
        .on_store(store)
        .execute()
        .map_err(|e| e.to_string())?
    else {
        return Err("not a SELECT query".to_string());
    };
    let header: Vec<String> = solutions
        .variables()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();
    let mut rows = Vec::new();
    for solution in solutions {
        let solution = solution.map_err(|e| e.to_string())?;
        rows.push(
            header
                .iter()
                .map(|v| show(solution.get(v.as_str())))
                .collect(),
        );
    }
    Ok((header, rows))
}

fn print_table(header: &[String], rows: &[Vec<String>]) {
    println!("{}", header.join("\t"));
    for row in rows {
        println!("{}", row.join("\t"));
    }
    println!("({} rows)", rows.len());
}

/// What the mapping argument named: the IRI to pass as `mapping=`, the queries to run,
/// and a file to bind when it was a path rather than a name.
fn mapping_argument(arg: &str) -> (String, PathBuf, Option<PathBuf>) {
    let as_path = Path::new(arg);
    if arg.contains(['/', '\\', '.']) || as_path.exists() {
        let queries = as_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("queries");
        return (
            MAPPING_IRI.to_string(),
            queries,
            Some(as_path.to_path_buf()),
        );
    }
    let home = MappingHome::ambient();
    let queries = home.queries_dir(arg).unwrap_or_else(|e| {
        eprintln!("mapping `{arg}`: {e}");
        std::process::exit(2);
    });
    (format!("urn:markdown:mapping:{arg}"), queries, None)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: reconcile <corpus-root> <mapping-name|mapping.ttl> [subdir…]");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]);
    let (mapping_iri, queries_dir, mapping_file) = mapping_argument(&args[1]);
    let subdirs: Vec<PathBuf> = if args.len() > 2 {
        args[2..].iter().map(|s| root.join(s)).collect()
    } else {
        vec![root.clone()]
    };
    let mut files = Vec::new();
    for dir in &subdirs {
        walk(dir, &mut files);
    }

    // A named mapping is served by the module's own space, from the config home; a
    // mapping file is bound here instead, under the same by-reference call.
    let mut space: EndpointSpace = ikigai_markdown::space();
    if let Some(path) = &mapping_file {
        space = space.bind(
            Exact::new(MAPPING_IRI),
            bound(
                "mapping",
                "text/turtle",
                std::fs::read(path).expect("read mapping"),
            ),
        );
    }
    let mut docs = Vec::new();
    for (i, file) in files.iter().enumerate() {
        let rel = file
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .to_string();
        let iri = format!("urn:corpus:file{i}");
        space = space.bind(
            Exact::new(iri.as_str()),
            bound(
                &format!("file{i}"),
                "text/markdown",
                std::fs::read(file).expect("read"),
            ),
        );
        docs.push((iri, rel));
    }
    let kernel = Kernel::new(Arc::new(space));
    let store = Store::new().expect("store");

    let started = Instant::now();
    let mut quads = 0usize;
    for (iri, rel) in &docs {
        let request = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
            .with_arg("src", ArgRef::Inline(iri.as_bytes().to_vec()))
            .with_arg("path", ArgRef::Inline(rel.as_bytes().to_vec()))
            .with_arg("mapping", ArgRef::Inline(mapping_iri.as_bytes().to_vec()));
        let repr = match futures::executor::block_on(kernel.issue(request, &Capability::root())) {
            Ok(repr) => repr,
            Err(e) => {
                eprintln!("lift failed for {rel}: {e}");
                std::process::exit(1);
            }
        };
        for quad in RdfParser::from_format(RdfFormat::NQuads).for_slice(&repr.bytes) {
            store
                .insert(&quad.expect("the lift emits valid N-Quads"))
                .unwrap();
            quads += 1;
        }
    }
    println!(
        "== lifted {} documents, {} quads, in {:.2?}",
        docs.len(),
        quads,
        started.elapsed()
    );

    let mut queries: Vec<PathBuf> = std::fs::read_dir(&queries_dir)
        .unwrap_or_else(|e| panic!("queries dir {}: {e}", queries_dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rq"))
        .collect();
    queries.sort();
    for q in queries {
        println!("\n== {}", q.file_name().unwrap().to_string_lossy());
        let text = std::fs::read_to_string(&q).expect("read query");
        let started = Instant::now();
        match select(&store, &text) {
            Ok((header, rows)) => {
                print_table(&header, &rows);
                println!("({:.2?})", started.elapsed());
            }
            Err(e) => {
                eprintln!("query {} failed: {e}", q.display());
                std::process::exit(1);
            }
        }
    }
}
