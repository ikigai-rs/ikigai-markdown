//! Pipeline citizenship, through the ENGINE — the only place it is visible.
//!
//! A Source fills the one declared argument left unnamed; if several are unnamed,
//! only the REQUIRED ones compete. `src` is the lift's one required argument, so a
//! piped document lands there, and `mapping=` / `path=` ride alongside by name.
//!
//! ⚠ The engine reads each contract as `Meta as=application/json` and fails OPEN
//! without it, so the kernel here has a JSON meta renderer, and the last test is the
//! witness that the contract is really being read.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    Description, EndpointSpace, Exact, FnEndpoint, Kernel, MetaRenderer, ReprType, Representation,
    Verb,
};
use ikigai_engine::{Action, Engine};

struct JsonRenderer;

impl MetaRenderer for JsonRenderer {
    fn render(
        &self,
        description: &Description,
        _target: &ReprType,
    ) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(
            ReprType::new("application/json"),
            serde_json::to_vec(description).expect("serialize description"),
        ))
    }
}

const NOTE: &str = "# A Note\n\nBody text.\n";

fn space() -> EndpointSpace {
    ikigai_markdown::space().bind(
        Exact::new("urn:test:note"),
        FnEndpoint::new("note", |_| {
            Ok(Representation::new(
                ReprType::new("text/markdown"),
                NOTE.as_bytes().to_vec(),
            ))
        })
        .with_description(
            Description::new("note")
                .title("A note")
                .summary("a markdown document for the pipeline tests")
                .verb(Verb::Source)
                .output("text/markdown"),
        ),
    )
}

fn run(engine: &Engine, line: &str) -> Result<String, String> {
    match block_on(engine.eval_async(line)) {
        Action::Output(entry) => entry.result,
        _ => Err(format!("`{line}` produced no output")),
    }
}

fn engine() -> Engine {
    Engine::new(Kernel::with_meta_renderer(
        Arc::new(space()),
        Arc::new(JsonRenderer),
    ))
}

#[test]
fn a_piped_document_is_lifted() {
    let out = run(
        &engine(),
        "source urn:test:note | urn:markdown:lift path=notes/a.md",
    )
    .expect("pipe");
    assert!(
        out.contains("<https://ikigai-rs.dev/ns/md#text> \"A Note\""),
        "{out}"
    );
    assert!(out.contains("<urn:markdown:doc:notes/a.md>"), "{out}");
}

#[test]
fn a_positional_reference_is_resolved() {
    let out = run(&engine(), "source urn:markdown:lift urn:test:note").expect("reference");
    // By reference with no path, the graph is the document's own IRI.
    assert!(
        out.contains("<https://ikigai-rs.dev/ns/md#source> <urn:test:note> <urn:test:note>"),
        "{out}"
    );
}

#[test]
fn the_tests_above_would_notice_a_routing_defect() {
    let blind = Engine::new(Kernel::new(Arc::new(space())));
    let err = run(&blind, "source urn:test:note | urn:markdown:lift")
        .expect_err("a blind engine cannot route the value");
    assert!(err.contains("src"), "the value never reached src: {err}");
}
