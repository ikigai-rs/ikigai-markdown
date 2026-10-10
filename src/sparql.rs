//! Every SPARQL text this module runs comes from a mapping, and a mapping is caller text:
//! `mapping=` carries it by value, or names a resource the caller may control. This is the
//! one place that text reaches a parser (ledger #963).
//!
//! oxigraph's SPARQL parser and evaluator are **recursive**, so a query's shape is a claim
//! on the stack of whatever thread runs it, and running out aborts the **whole process** —
//! every request the host is serving, not this one. Measured on `main` before this module
//! existed, on a 2 MiB thread (a tokio worker's): a `md:construct` nesting 3,000 parentheses,
//! a 3,000-long run of `!`, or a FLAT 600-term `||` chain each aborted the host through
//! `urn:markdown:lift` (`tests/sparql_stack.rs`).
//!
//! The bounds are `ikigai-store`'s, not a copy of them (ledger #976 is where they become
//! one shared home):
//!
//! 1. [`limits::check_sparql`] refuses text past `MAX_SPARQL_BYTES` or nesting deeper than
//!    `MAX_SPARQL_NESTING`, before the parser sees a byte.
//! 2. [`limits::on_sparql_stack`] runs the parse AND the evaluation — the evaluator is lazy,
//!    so that includes draining the triples — on a thread sized from the text's length. On
//!    wasm there is no thread and only the first layer applies.
//! 3. [`budget::check_query`] refuses an algebra past `MAX_JOIN_OPERANDS` in one join or
//!    `MAX_ALGEBRA_NODES` in all, before oxigraph PLANS it. That is a bound on TIME, not
//!    stack: the planner cannot be interrupted. Measured through `urn:markdown:lift` on a
//!    release build without it, over a three-line document: 250 triple patterns took 66 s,
//!    a 1,000-step path 70 s, a 10,000-term `||` chain 4.8 s — every one inside the first
//!    two bounds, and each planned twice (once to check the mapping, once to apply it).
//!    Each is now refused in milliseconds.
//!
//! ⚠ **There is still no DEADLINE on the evaluation.** What the algebra bound leaves is the
//! shape `ikigai-store` caps by counting rather than stopping: a cross product. A 90-byte
//! construct of four unrelated patterns over a twenty-paragraph document was still running
//! after five minutes, and the DOCUMENT is caller text too. oxigraph's cancellation token
//! is not checked inside a cross product's loops, so a deadline here would answer the
//! caller on time but not stop the work; the store's machinery for that (a sized thread,
//! the token, an overdue count) is private to it, and a copy is not a small change. It is
//! ledger #976's to share.
//!
//! 4. ★ **No `SERVICE`, in any build** (ledger #1085, #1083). A host whose graph enables
//!    `oxigraph/http-client` (rudof does, so `ikigai-cli` does) gets oxigraph's default HTTP
//!    service handler on every plain `SparqlEvaluator`, and a `SERVICE <http://…>` in a
//!    mapping became an outbound request no `urn:cap:net:*` gates: measured through
//!    `urn:markdown:lift` against a 127.0.0.1 stub, four lifts sent it 27 requests
//!    (`tests/service_egress.rs`). [`service::refuse_service_with`] refuses one anywhere in
//!    the algebra (`EXISTS`, `OPTIONAL`, `LATERAL`, a variable name, `SILENT`) before
//!    evaluation, worded for a mapping rather than the store (ledger #1108), and
//!    [`service::evaluator`] refuses the call itself behind it. `LOAD` needs no check
//!    here: it is an UPDATE, and this module only ever parses a QUERY, so a `LOAD` is refused
//!    as not SPARQL before anything evaluates.
//!
//! Every refusal is a typed [`Error::InvalidArgument`] on `mapping`, the argument that
//! carried the text.

use ikigai_core::{Error, Result};
use ikigai_store::{budget, limits, service};
use oxigraph::model::Triple;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

/// The argument every mapping's SPARQL arrives in.
pub(crate) const ARG: &str = "mapping";

/// What a mapping author does instead of a `SERVICE`, after the ecosystem's shared first
/// sentence ([`service::SERVICE_REFUSAL`]). Not the store's own remedy, which tells the caller
/// to sink into `urn:iki:store:load` — wrong advice inside a mapping (ledger #1108).
const SERVICE_REMEDY: &str = "A mapping's constructs read only the document being lifted: \
     fetch remote data through the kernel, where the network capability applies, and pass it \
     in as a document of its own.";

/// A refusal on [`ARG`].
pub(crate) fn refuse(detail: impl Into<String>) -> Error {
    Error::InvalidArgument {
        name: ARG.to_string(),
        detail: detail.into(),
    }
}

/// Parse `text` as a CONSTRUCT and evaluate it over `store`'s default graph, returning its
/// triples — or `None` when it parses but is not a CONSTRUCT. `what` names the query in
/// every refusal (`md:construct`, `construct 2`).
pub(crate) fn construct(text: &str, store: &Store, what: &str) -> Result<Option<Vec<Triple>>> {
    let bounded = |e: Error| match e {
        Error::InvalidArgument { detail, .. } => refuse(format!("{what}: {detail}")),
        other => other,
    };
    limits::check_sparql(text, ARG).map_err(bounded)?;
    limits::on_sparql_stack(text, || {
        let query = spargebra::SparqlParser::new()
            .parse_query(text)
            .map_err(|e| refuse(format!("{what} is not valid SPARQL: {e}")))?;
        budget::check_query(&query, ARG).map_err(bounded)?;
        service::refuse_service_with(&query, ARG, Some(SERVICE_REMEDY)).map_err(bounded)?;
        let results = service::evaluator()
            .for_query(query)
            .on_store(store)
            .execute()
            .map_err(|e| refuse(format!("{what} does not evaluate: {e}")))?;
        let QueryResults::Graph(triples) = results else {
            return Ok(None);
        };
        triples
            .map(|t| t.map_err(|e| refuse(format!("{what}: {e}"))))
            .collect::<Result<Vec<_>>>()
            .map(Some)
    })
}

/// The detail of a refusal, for the public API that predates typed errors and returns
/// `String`; anything else as it displays.
pub(crate) fn detail(e: Error) -> String {
    match e {
        Error::InvalidArgument { detail, .. } => detail,
        other => other.to_string(),
    }
}
