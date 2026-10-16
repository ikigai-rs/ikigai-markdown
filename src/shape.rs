//! The shape a mapping STATES for what its constructs write, and checking output against it.
//!
//! A mapping may point at one or more SHACL node shapes with `md:shape`, written in the
//! mapping's own Turtle:
//!
//! ```turtle
//! <urn:ex:mapping> a md:Mapping ; md:shape <urn:ex:mapping:shape:note> ; … .
//! <urn:ex:mapping:shape:note> a sh:NodeShape ; sh:targetClass ex:Note ;
//!     sh:property [ sh:path ex:title ; sh:minCount 1 ; sh:maxCount 1 ;
//!                   sh:datatype xsd:string ] .
//! ```
//!
//! The linter checks a SUBSET of SHACL Core, and says so rather than pretending: focus
//! nodes by `sh:targetClass`, and on each `sh:property` whose `sh:path` is one predicate,
//! `sh:minCount`, `sh:maxCount`, `sh:datatype`, `sh:nodeKind`, `sh:class` and `sh:in`.
//! Anything else in the `sh:` namespace is reported as not checked, never silently passed.
//! A full SHACL engine is `ikigai-shacl`'s, which a host can run over the same output.

use std::collections::{BTreeMap, BTreeSet};

use oxigraph::model::{NamedOrBlankNode, Term, Triple};

use crate::mapping::RawMapping;
use crate::vocab::{md, RDF_TYPE};

/// The SHACL namespace.
pub(crate) const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// One property constraint the linter checks.
#[derive(Debug, Clone, Default)]
pub(crate) struct Property {
    pub path: String,
    pub min: Option<u64>,
    pub max: Option<u64>,
    pub datatype: Option<String>,
    pub node_kind: Option<String>,
    pub class: Option<String>,
    pub one_of: Option<Vec<Term>>,
}

/// One node shape.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shape {
    pub name: String,
    pub targets: Vec<String>,
    pub properties: Vec<Property>,
}

/// The shapes a mapping states, and what in them the linter does not check.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shapes {
    pub shapes: Vec<Shape>,
    pub unchecked: Vec<String>,
}

impl Shapes {
    /// Every predicate some shape declares a property for.
    pub fn declared_predicates(&self) -> BTreeSet<&str> {
        self.shapes
            .iter()
            .flat_map(|s| s.properties.iter().map(|p| p.path.as_str()))
            .collect()
    }

    /// Every class some shape targets.
    pub fn declared_classes(&self) -> BTreeSet<&str> {
        self.shapes
            .iter()
            .flat_map(|s| s.targets.iter().map(String::as_str))
            .collect()
    }
}

fn node_name(n: &NamedOrBlankNode) -> String {
    match n {
        NamedOrBlankNode::NamedNode(n) => crate::analyze::short(n.as_str()),
        NamedOrBlankNode::BlankNode(_) => "a blank-node shape".to_string(),
    }
}

fn as_node(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

/// The shapes `raw` states with `md:shape`, or `None` when it states none.
pub(crate) fn read(raw: &RawMapping) -> Option<Shapes> {
    let triples = &raw.triples;
    let about = |s: &NamedOrBlankNode| -> Vec<&Triple> {
        triples.iter().filter(|t| &t.subject == s).collect()
    };
    let shape_p = md("shape");
    let roots: Vec<NamedOrBlankNode> = triples
        .iter()
        .filter(|t| t.subject == raw.subject && t.predicate == shape_p)
        .filter_map(|t| as_node(&t.object))
        .collect();
    if roots.is_empty() {
        return None;
    }
    let mut out = Shapes::default();
    for root in roots {
        let name = node_name(&root);
        let mut shape = Shape {
            name: name.clone(),
            ..Shape::default()
        };
        for t in about(&root) {
            let p = t.predicate.as_str();
            let Some(local) = p.strip_prefix(SH) else {
                continue;
            };
            match (local, &t.object) {
                ("targetClass", Term::NamedNode(c)) => shape.targets.push(c.as_str().to_string()),
                ("property", object) => match as_node(object) {
                    Some(node) => {
                        if let Some(property) = read_property(&name, &node, triples, &mut out) {
                            shape.properties.push(property);
                        }
                    }
                    None => out
                        .unchecked
                        .push(format!("{name}: an sh:property that is not a node")),
                },
                ("name" | "description" | "message" | "deactivated" | "severity", _) => {}
                (other, _) => out
                    .unchecked
                    .push(format!("{name}: sh:{other} is not checked by this linter")),
            }
        }
        if shape.targets.is_empty() {
            out.unchecked.push(format!(
                "{name} has no sh:targetClass, so the linter has no focus nodes for it"
            ));
        }
        shape.targets.sort();
        shape.properties.sort_by(|a, b| a.path.cmp(&b.path));
        out.shapes.push(shape);
    }
    out.shapes.sort_by(|a, b| a.name.cmp(&b.name));
    out.unchecked.sort();
    out.unchecked.dedup();
    Some(out)
}

fn read_property(
    shape: &str,
    node: &NamedOrBlankNode,
    triples: &[Triple],
    out: &mut Shapes,
) -> Option<Property> {
    let mut property = Property::default();
    let mut path = None;
    for t in triples.iter().filter(|t| &t.subject == node) {
        let Some(local) = t.predicate.as_str().strip_prefix(SH) else {
            continue;
        };
        let count = |t: &Term| match t {
            Term::Literal(l) => l.value().parse::<u64>().ok(),
            _ => None,
        };
        let iri = |t: &Term| match t {
            Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        };
        match local {
            "path" => match &t.object {
                Term::NamedNode(n) => path = Some(n.as_str().to_string()),
                _ => {
                    out.unchecked.push(format!(
                        "{shape}: a property path that is not one predicate is not checked by \
                         this linter"
                    ));
                    return None;
                }
            },
            "minCount" => property.min = count(&t.object),
            "maxCount" => property.max = count(&t.object),
            "datatype" => property.datatype = iri(&t.object),
            "nodeKind" => property.node_kind = iri(&t.object),
            "class" => property.class = iri(&t.object),
            "in" => property.one_of = Some(list(&t.object, triples)),
            "name" | "description" | "message" | "order" | "group" | "severity" | "deactivated" => {
            }
            other => out.unchecked.push(format!(
                "{shape}: sh:{other} on a property is not checked by this linter"
            )),
        }
    }
    match path {
        Some(path) => {
            property.path = path;
            Some(property)
        }
        None => {
            out.unchecked
                .push(format!("{shape}: an sh:property with no sh:path"));
            None
        }
    }
}

/// An RDF list's members, stopping at a cycle or a malformed cell.
fn list(head: &Term, triples: &[Triple]) -> Vec<Term> {
    let first = format!("{RDF}first");
    let rest = format!("{RDF}rest");
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cell = as_node(head);
    while let Some(c) = cell.take() {
        if !seen.insert(c.to_string()) {
            break;
        }
        for t in triples.iter().filter(|t| t.subject == c) {
            if t.predicate.as_str() == first {
                out.push(t.object.clone());
            } else if t.predicate.as_str() == rest {
                cell = as_node(&t.object);
            }
        }
    }
    out
}

/// One value or count that does not conform.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Violation {
    /// The focus node, as an IRI string.
    pub focus: String,
    pub message: String,
}

/// Check `triples` (a domain graph) against `shapes`. Sorted, deterministic.
pub(crate) fn validate(shapes: &Shapes, triples: &[Triple]) -> Vec<Violation> {
    let mut values: BTreeMap<(String, &str), Vec<&Term>> = BTreeMap::new();
    let mut types: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for t in triples {
        let s = t.subject.to_string();
        if t.predicate.as_str() == RDF_TYPE {
            if let Term::NamedNode(c) = &t.object {
                types.entry(s.clone()).or_default().insert(c.as_str());
            }
        }
        values
            .entry((s, t.predicate.as_str()))
            .or_default()
            .push(&t.object);
    }
    let short = crate::analyze::short;
    let mut out = BTreeSet::new();
    for shape in &shapes.shapes {
        for target in &shape.targets {
            for (focus, classes) in &types {
                if !classes.contains(target.as_str()) {
                    continue;
                }
                let focus_iri = focus
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string();
                for p in &shape.properties {
                    let found = values
                        .get(&(focus.clone(), p.path.as_str()))
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let mut violation = |message: String| {
                        out.insert(Violation {
                            focus: focus_iri.clone(),
                            message: format!("{} {}: {message}", short(&focus_iri), short(&p.path)),
                        });
                    };
                    let n = found.len() as u64;
                    if let Some(min) = p.min.filter(|m| n < *m) {
                        violation(format!(
                            "{n} value(s), and {} asks for at least {min} (sh:minCount)",
                            shape.name
                        ));
                    }
                    if let Some(max) = p.max.filter(|m| n > *m) {
                        violation(format!(
                            "{n} values, and {} allows at most {max} (sh:maxCount)",
                            shape.name
                        ));
                    }
                    for v in found {
                        if let Some(dt) = &p.datatype {
                            let ok = matches!(v, Term::Literal(l) if l.datatype().as_str() == dt);
                            if !ok {
                                violation(format!("{v} is not a {} (sh:datatype)", short(dt)));
                            }
                        }
                        if let Some(kind) = &p.node_kind {
                            if !node_kind(v, kind) {
                                violation(format!("{v} is not a {} (sh:nodeKind)", short(kind)));
                            }
                        }
                        if let Some(class) = &p.class {
                            let ok = types
                                .get(&v.to_string())
                                .is_some_and(|c| c.contains(class.as_str()));
                            if !ok {
                                violation(format!("{v} is not typed {} (sh:class)", short(class)));
                            }
                        }
                        if let Some(allowed) = &p.one_of {
                            if !allowed.iter().any(|a| a == *v) {
                                violation(format!("{v} is not one of the sh:in values"));
                            }
                        }
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

fn node_kind(v: &Term, kind: &str) -> bool {
    let (iri, blank, literal) = (
        matches!(v, Term::NamedNode(_)),
        matches!(v, Term::BlankNode(_)),
        matches!(v, Term::Literal(_)),
    );
    match kind.strip_prefix(SH) {
        Some("IRI") => iri,
        Some("BlankNode") => blank,
        Some("Literal") => literal,
        Some("BlankNodeOrIRI") => blank || iri,
        Some("BlankNodeOrLiteral") => blank || literal,
        Some("IRIOrLiteral") => iri || literal,
        _ => true,
    }
}
