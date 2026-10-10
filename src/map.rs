//! Stage 2: run a mapping's CONSTRUCTs over one document's structural graph.

use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term};
use oxigraph::store::Store;

use crate::sparql::refuse;

/// Run `constructs` over `structural` (queried as the DEFAULT graph, so a mapping
/// needs no `GRAPH` clause) and return their union as quads in `graph`.
///
/// Refuses a construct that emits a blank node: the module recipe skolemizes, and
/// a blank node minted per run would make two lifts of the same bytes differ. Each
/// construct is caller text, so it runs inside the bounds `crate::sparql` applies
/// (ledger #963).
pub fn apply(
    structural: &[Quad],
    graph: &NamedNode,
    constructs: &[String],
) -> Result<Vec<Quad>, String> {
    apply_checked(structural, graph, constructs).map_err(crate::sparql::detail)
}

/// [`apply`] with its refusals typed, as [`crate::Mapping::parse`]'s twin is.
pub(crate) fn apply_checked(
    structural: &[Quad],
    graph: &NamedNode,
    constructs: &[String],
) -> ikigai_core::Result<Vec<Quad>> {
    if constructs.is_empty() {
        return Ok(Vec::new());
    }
    let store_error = |e: oxigraph::store::StorageError| refuse(format!("in-memory store: {e}"));
    let store = Store::new().map_err(store_error)?;
    for q in structural {
        store
            .insert(&Quad::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
                GraphName::DefaultGraph,
            ))
            .map_err(store_error)?;
    }
    let mut out = Vec::new();
    for (i, text) in constructs.iter().enumerate() {
        let what = format!("construct {}", i + 1);
        let Some(triples) = crate::sparql::construct(text, &store, &what)? else {
            return Err(refuse(format!("{what} is not a CONSTRUCT query")));
        };
        for t in triples {
            let subject = match t.subject {
                NamedOrBlankNode::NamedNode(n) => n,
                _ => {
                    return Err(refuse(format!(
                        "{what} emitted a blank-node subject; build IRIs with IRI(CONCAT(…))"
                    )))
                }
            };
            if matches!(t.object, Term::BlankNode(_)) {
                return Err(refuse(format!(
                    "{what} emitted a blank-node object; build IRIs with IRI(CONCAT(…))"
                )));
            }
            out.push(Quad::new(
                subject,
                t.predicate,
                t.object,
                GraphName::NamedNode(graph.clone()),
            ));
        }
    }
    Ok(out)
}

/// Serialize quads deterministically: sorted, de-duplicated N-Quads lines, or
/// TriG written in that same order.
pub fn serialize(quads: &[Quad], trig: bool) -> Result<Vec<u8>, String> {
    let mut sorted: Vec<&Quad> = quads.iter().collect();
    sorted.sort_by_cached_key(|q| q.to_string());
    sorted.dedup();
    if !trig {
        let mut out = String::new();
        for q in sorted {
            out.push_str(&q.to_string());
            out.push_str(" .\n");
        }
        return Ok(out.into_bytes());
    }
    let mut writer = RdfSerializer::from_format(RdfFormat::TriG)
        .with_prefix("md", crate::vocab::MD)
        .map_err(|e| e.to_string())?
        .for_writer(Vec::new());
    for q in sorted {
        writer.serialize_quad(q).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())
}
