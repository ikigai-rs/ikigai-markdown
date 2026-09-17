//! Shared test harness: bind files as kernel resources, lift them BY REFERENCE
//! through `urn:markdown:lift` (the real call a host makes), load the N-Quads into
//! an in-memory store, and run SELECT queries over the union of every graph.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::Term;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;

pub const MAPPING_IRI: &str = "urn:test:mapping";

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The example mapping's directory, laid out exactly as one installed in the config
/// home: `mapping.ttl` beside `queries/`.
pub fn example_dir() -> PathBuf {
    repo().join("mappings/example")
}

pub fn mapping_path() -> PathBuf {
    example_dir().join("mapping.ttl")
}

pub fn query(name: &str) -> String {
    std::fs::read_to_string(example_dir().join("queries").join(name))
        .unwrap_or_else(|e| panic!("read query {name}: {e}"))
}

/// A scratch config home, removed on drop.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "ikigai-markdown-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Install the example mapping in this home under `name`, as a person would:
    /// copy the directory.
    pub fn install_example(&self, name: &str) -> PathBuf {
        let dir = self.0.join("markdown/mappings").join(name);
        std::fs::create_dir_all(dir.join("queries")).unwrap();
        std::fs::copy(mapping_path(), dir.join("mapping.ttl")).unwrap();
        for entry in std::fs::read_dir(example_dir().join("queries")).unwrap() {
            let from = entry.unwrap().path();
            std::fs::copy(&from, dir.join("queries").join(from.file_name().unwrap())).unwrap();
        }
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn file(id: &str, media: &'static str, bytes: Vec<u8>) -> FnEndpoint {
    FnEndpoint::new(id.to_string(), move |_| {
        Ok(Representation::new(ReprType::new(media), bytes.clone()))
    })
    .with_description(
        Description::new(id)
            .title("Test file")
            .summary("a file bound for a test")
            .verb(Verb::Source)
            .output(media),
    )
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "md") {
            out.push(p);
        }
    }
}

/// The example corpus as (relative path, text), with `edits` replacing or adding
/// documents by relative path.
pub fn corpus(edits: &[(&str, &str)]) -> Vec<(String, String)> {
    let root = repo().join("tests/fixtures/example");
    let mut files = Vec::new();
    walk(&root.join("decisions"), &mut files);
    let mut docs: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            (
                p.strip_prefix(&root).unwrap().to_string_lossy().to_string(),
                std::fs::read_to_string(p).unwrap(),
            )
        })
        .collect();
    for (path, text) in edits {
        match docs.iter_mut().find(|(p, _)| p == path) {
            Some(doc) => doc.1 = text.to_string(),
            None => docs.push((path.to_string(), text.to_string())),
        }
    }
    docs
}

/// Lift every document through the kernel under `mapping` (Turtle, bound at
/// [`MAPPING_IRI`]) and load the result.
pub fn load(docs: &[(String, String)], mapping: &str) -> Store {
    let space = ikigai_markdown::space().bind(
        Exact::new(MAPPING_IRI),
        file("mapping", "text/turtle", mapping.as_bytes().to_vec()),
    );
    load_in(space, docs, MAPPING_IRI)
}

/// Lift every document through `space`'s kernel, passing `mapping_iri` BY REFERENCE,
/// and load the result.
pub fn load_in(mut space: EndpointSpace, docs: &[(String, String)], mapping_iri: &str) -> Store {
    for (i, (_, text)) in docs.iter().enumerate() {
        let iri = format!("urn:test:doc{i}");
        space = space.bind(
            Exact::new(iri.as_str()),
            file(
                &format!("doc{i}"),
                "text/markdown",
                text.as_bytes().to_vec(),
            ),
        );
    }
    let kernel = Kernel::new(Arc::new(space));
    let store = Store::new().unwrap();
    for (i, (path, _)) in docs.iter().enumerate() {
        let request = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
            .with_arg(
                "src",
                ArgRef::Inline(format!("urn:test:doc{i}").into_bytes()),
            )
            .with_arg("path", ArgRef::Inline(path.as_bytes().to_vec()))
            .with_arg("mapping", ArgRef::Inline(mapping_iri.as_bytes().to_vec()));
        let repr = futures::executor::block_on(kernel.issue(request, &Capability::root()))
            .unwrap_or_else(|e| panic!("lift {path}: {e}"));
        for quad in RdfParser::from_format(RdfFormat::NQuads).for_slice(&repr.bytes) {
            store.insert(&quad.unwrap()).unwrap();
        }
    }
    store
}

pub fn fixture_store() -> Store {
    load(
        &corpus(&[]),
        &std::fs::read_to_string(mapping_path()).unwrap(),
    )
}

fn show(term: Option<&Term>) -> String {
    match term {
        None => String::new(),
        Some(Term::Literal(l)) => l.value().to_string(),
        Some(Term::NamedNode(n)) => n.as_str().to_string(),
        Some(other) => other.to_string(),
    }
}

/// Run a SELECT over the union of every graph; rows as display strings, in the
/// query's own variable order.
pub fn select(store: &Store, query: &str) -> Vec<Vec<String>> {
    let mut evaluator = SparqlEvaluator::new().parse_query(query).unwrap();
    evaluator.dataset_mut().set_default_graph_as_union();
    let QueryResults::Solutions(solutions) = evaluator.on_store(store).execute().unwrap() else {
        panic!("not a SELECT");
    };
    let vars: Vec<String> = solutions
        .variables()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();
    solutions
        .map(|s| {
            let s = s.unwrap();
            vars.iter().map(|v| show(s.get(v.as_str()))).collect()
        })
        .collect()
}

/// Rows projected to the named columns, for assertions that ignore the rest.
pub fn columns(store: &Store, query: &str, wanted: &[&str]) -> Vec<Vec<String>> {
    let mut evaluator = SparqlEvaluator::new().parse_query(query).unwrap();
    evaluator.dataset_mut().set_default_graph_as_union();
    let QueryResults::Solutions(solutions) = evaluator.on_store(store).execute().unwrap() else {
        panic!("not a SELECT");
    };
    solutions
        .map(|s| {
            let s = s.unwrap();
            wanted.iter().map(|v| show(s.get(*v))).collect()
        })
        .collect()
}

pub fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|c| c.to_string()).collect()
}
