//! Check 2: a corpus lifted NOW against a BASELINE, per document.
//!
//! Both sides are snapshots in the same format: the N-Quads `urn:markdown:lint:snapshot` answers,
//! one named graph per document. The baseline is a snapshot saved earlier (a file, or any
//! resource that serves one). It is not a git revision: a snapshot is a resource, so it is
//! read through the kernel like everything else, needs no repository, and records what the
//! lift actually produced then. A host that serves files at a revision can serve a
//! snapshot of one too.
//!
//! Each graph is split in two by vocabulary: a triple whose predicate is an `md:` term (or
//! that types a node with an `md:` class) is STRUCTURE, what the lift saw; anything else is
//! what the mapping PRODUCED. A construct that writes `md:` terms blurs that line, and the
//! mapping check warns about it.
//!
//! What it reports, per document:
//!
//! - records the mapping produced at the baseline and no longer does, or now newly does;
//! - fields (a record's predicate and value) lost, gained, or changed (one value replaced
//!   by one other);
//! - citations no longer recognized, or newly recognized, counted per pattern and value;
//! - headings the mapping reads (by text or by level) that vanished, or moved to another
//!   level or another parent section;
//! - frontmatter keys the mapping does not read (a warning when new since the baseline),
//!   keys it reads that are gone, and frontmatter that no longer parses;
//!
//! and for the corpus: documents gone or new, and every value that does not conform to the
//! shape the mapping states (an error when new since the baseline).
//!
//! A node the lift minted inside a document (`…#m3`, a citation match) is named by what it
//! IS, `[match decision "0007"]`, not by its number, so a document that gains a paragraph
//! does not renumber every record below it into a loss and a gain.

use std::collections::{BTreeMap, BTreeSet};

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, Literal, NamedOrBlankNode, Quad, Term, Triple};

use crate::analyze::short;
use crate::lint::{Expectations, Finding, Report, Severity};
use crate::shape::Shapes;
use crate::vocab::{MD, RDF_TYPE};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Heading {
    text: String,
    level: i64,
    order: i64,
    parent: Option<String>,
}

/// One document's graph, read back.
#[derive(Debug, Default)]
struct Doc {
    graph: String,
    path: String,
    headings: Vec<Heading>,
    keys: BTreeSet<String>,
    frontmatter_error: Option<String>,
    /// Why the lift could not read the document at all, when it could not.
    lift_error: Option<String>,
    /// (pattern name, value) → [(line, matched text)].
    matches: BTreeMap<(String, String), Vec<(i64, String)>>,
    /// subject → predicate → objects (a sorted multiset), all named for display.
    produced: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// What the mapping produced, as triples, for the shape.
    triples: Vec<Triple>,
}

/// Read a snapshot: N-Quads, one named graph per document.
fn parse(bytes: &[u8], what: &str) -> Result<BTreeMap<String, Vec<Quad>>, String> {
    let mut graphs: BTreeMap<String, Vec<Quad>> = BTreeMap::new();
    for quad in RdfParser::from_format(RdfFormat::NQuads).for_slice(bytes) {
        let quad = quad.map_err(|e| format!("{what} is not N-Quads: {e}"))?;
        let GraphName::NamedNode(g) = &quad.graph_name else {
            return Err(format!(
                "{what} holds a triple outside any named graph, so it is not a snapshot \
                 urn:markdown:lint:snapshot wrote: {quad}"
            ));
        };
        graphs.entry(g.as_str().to_string()).or_default().push(quad);
    }
    Ok(graphs)
}

/// One document's lift reduced to what the comparison reads: everything the mapping
/// produced, and of the structure only the document's path and frontmatter error, its
/// headings, frontmatter keys and citation matches, and the type and text of any node the
/// produced triples point at. A full lift of a long document is mostly paragraph text no
/// check reads, and a corpus of them would not fit a snapshot bound; this keeps a snapshot
/// in proportion to what can change the answer. Comparing two reduced lifts gives exactly
/// what comparing the two full ones does.
pub(crate) fn reduce(quads: Vec<Quad>) -> Vec<Quad> {
    let md = |local: &str| format!("{MD}{local}");
    let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for q in &quads {
        if q.predicate.as_str() == RDF_TYPE {
            if let Term::NamedNode(c) = &q.object {
                if let Some(local) = c.as_str().strip_prefix(MD) {
                    classes
                        .entry(subject_key(&q.subject))
                        .or_default()
                        .insert(local.to_string());
                }
            }
        }
        if !structural(q) {
            referenced.insert(subject_key(&q.subject));
            if let Term::NamedNode(n) = &q.object {
                referenced.insert(n.as_str().to_string());
            }
        }
    }
    let keep = |q: &Quad| -> bool {
        if !structural(q) {
            return true;
        }
        let s = subject_key(&q.subject);
        let p = q.predicate.as_str();
        if p == RDF_TYPE {
            return true;
        }
        let is = |class: &str| classes.get(&s).is_some_and(|c| c.contains(class));
        let one_of = |locals: &[&str]| locals.iter().any(|l| p == md(l));
        (is("Document") && one_of(&["path", "frontmatterError", "liftError"]))
            || (is("Heading") && one_of(&["text", "level", "order", "section"]))
            || (is("Field") && one_of(&["key"]))
            || (is("Match") && one_of(&["matchedBy", "value", "line", "text"]))
            || (referenced.contains(&s) && one_of(&["text", "matchedBy", "value"]))
    };
    let typed_kept = |q: &Quad| -> bool {
        // A type is kept only on a node something else of it is kept for.
        let s = subject_key(&q.subject);
        let is = |class: &str| classes.get(&s).is_some_and(|c| c.contains(class));
        is("Document") || is("Heading") || is("Field") || is("Match") || referenced.contains(&s)
    };
    let text = md("text");
    quads
        .into_iter()
        .filter(|q| {
            keep(q) && (q.predicate.as_str() != RDF_TYPE || !structural(q) || typed_kept(q))
        })
        .map(|q| {
            // A block's text is only ever read to NAME the block, by its first characters
            // (see `read_doc`), so the snapshot keeps those and not a paragraph per citation.
            let s = subject_key(&q.subject);
            let named_only = !classes
                .get(&s)
                .is_some_and(|c| c.contains("Heading") || c.contains("Match"));
            match &q.object {
                Term::Literal(l) if named_only && q.predicate.as_str() == text => {
                    let cut: String = l.value().chars().take(NAME_CHARS).collect();
                    Quad::new(q.subject, q.predicate, Literal::from(cut), q.graph_name)
                }
                _ => q,
            }
        })
        .collect()
}

/// How many characters of a block's text name it in a report.
const NAME_CHARS: usize = 40;

fn structural(q: &Quad) -> bool {
    q.predicate.as_str().starts_with(MD)
        || (q.predicate.as_str() == RDF_TYPE
            && matches!(&q.object, Term::NamedNode(n) if n.as_str().starts_with(MD)))
}

fn subject_key(s: &NamedOrBlankNode) -> String {
    match s {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
    }
}

fn literal(t: &Term) -> Option<&str> {
    match t {
        Term::Literal(l) => Some(l.value()),
        _ => None,
    }
}

fn read_doc(graph: &str, quads: &[Quad], ignore: &BTreeSet<&str>) -> Doc {
    let md = |local: &str| format!("{MD}{local}");
    let mut props: BTreeMap<String, Vec<(&str, &Term)>> = BTreeMap::new();
    for q in quads.iter().filter(|q| structural(q)) {
        props
            .entry(subject_key(&q.subject))
            .or_default()
            .push((q.predicate.as_str(), &q.object));
    }
    let get = |s: &str, p: &str| -> Option<&Term> {
        props
            .get(s)
            .and_then(|ps| ps.iter().find(|(pp, _)| *pp == p).map(|(_, o)| *o))
    };
    let typed = |s: &str, class: &str| -> bool {
        let class = md(class);
        props.get(s).is_some_and(|ps| {
            ps.iter().any(|(p, o)| {
                *p == RDF_TYPE && matches!(o, Term::NamedNode(n) if n.as_str() == class)
            })
        })
    };
    let text_of = |s: &str| get(s, &md("text")).and_then(literal).map(str::to_string);
    let int_of = |s: &str, p: &str| {
        get(s, &md(p))
            .and_then(literal)
            .and_then(|v| v.parse::<i64>().ok())
    };

    let mut doc = Doc {
        graph: graph.to_string(),
        path: get(graph, &md("path"))
            .and_then(literal)
            .unwrap_or(graph)
            .to_string(),
        frontmatter_error: get(graph, &md("frontmatterError"))
            .and_then(literal)
            .map(str::to_string),
        lift_error: get(graph, &md("liftError"))
            .and_then(literal)
            .map(str::to_string),
        ..Doc::default()
    };
    for s in props.keys() {
        if typed(s, "Heading") {
            let parent = get(s, &md("section")).and_then(|p| match p {
                Term::NamedNode(n) => text_of(n.as_str()),
                _ => None,
            });
            doc.headings.push(Heading {
                text: text_of(s).unwrap_or_default(),
                level: int_of(s, "level").unwrap_or(0),
                order: int_of(s, "order").unwrap_or(0),
                parent,
            });
        }
        if typed(s, "Field") {
            if let Some(k) = get(s, &md("key")).and_then(literal) {
                doc.keys.insert(k.to_string());
            }
        }
        if typed(s, "Match") {
            let name = get(s, &md("matchedBy")).and_then(literal).unwrap_or("");
            let value = get(s, &md("value")).and_then(literal).unwrap_or("");
            let text = text_of(s).unwrap_or_default();
            doc.matches
                .entry((name.to_string(), value.to_string()))
                .or_default()
                .push((int_of(s, "line").unwrap_or(0), text));
        }
    }
    doc.headings.sort_by_key(|h| h.order);
    for m in doc.matches.values_mut() {
        m.sort();
    }

    // A node the lift minted for this document is named by what it is, not its number.
    let local_base = if graph.contains('#') {
        format!("{graph}-")
    } else {
        format!("{graph}#")
    };
    let name = |iri: &str| -> String {
        if !iri.starts_with(&local_base) {
            return short(iri);
        }
        if typed(iri, "Match") {
            let by = get(iri, &md("matchedBy")).and_then(literal).unwrap_or("");
            let value = get(iri, &md("value")).and_then(literal).unwrap_or("");
            return format!("[match {by} {value:?}]");
        }
        let class = props.get(iri).and_then(|ps| {
            ps.iter().find_map(|(p, o)| match o {
                Term::NamedNode(n) if *p == RDF_TYPE => n.as_str().strip_prefix(MD),
                _ => None,
            })
        });
        match (class, text_of(iri)) {
            (Some(c), Some(t)) => {
                let t: String = t.chars().take(NAME_CHARS).collect();
                format!("[{c} {t:?}]")
            }
            (Some(c), None) => format!("[{c}]"),
            _ => "[a node of this document]".to_string(),
        }
    };
    for q in quads.iter().filter(|q| !structural(q)) {
        // The shape sees everything the mapping produced; `ignore` is for the comparison.
        doc.triples.push(Triple::new(
            q.subject.clone(),
            q.predicate.clone(),
            q.object.clone(),
        ));
        if ignore.contains(q.predicate.as_str()) {
            continue;
        }
        let subject = match &q.subject {
            NamedOrBlankNode::NamedNode(n) => name(n.as_str()),
            other => other.to_string(),
        };
        let object = match &q.object {
            Term::NamedNode(n) => name(n.as_str()),
            other => other.to_string(),
        };
        doc.produced
            .entry(subject)
            .or_default()
            .entry(q.predicate.as_str().to_string())
            .or_default()
            .push(object);
    }
    for ps in doc.produced.values_mut() {
        for os in ps.values_mut() {
            os.sort();
        }
    }
    doc
}

/// `a - b` for sorted multisets.
fn minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut left = b.to_vec();
    let mut out = Vec::new();
    for x in a {
        match left.iter().position(|y| y == x) {
            Some(i) => {
                left.remove(i);
            }
            None => out.push(x.clone()),
        }
    }
    out
}

fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// Compare `current` (a snapshot) with `baseline` (one, or none) under the mapping read as
/// `expects` and `shapes`. Predicates in `ignore` are left out of the record comparison
/// (a line number a construct copies onto every record moves whenever a line is added
/// above it). `Err` is a snapshot that is not one.
pub(crate) fn compare(
    expects: Option<&Expectations>,
    shapes: Option<&Shapes>,
    current: &[u8],
    baseline: Option<&[u8]>,
    ignore: &[String],
) -> Result<Report, String> {
    let ignore: BTreeSet<&str> = ignore.iter().map(String::as_str).collect();
    let now: BTreeMap<String, Doc> = parse(current, "the current lift")?
        .iter()
        .map(|(g, qs)| (g.clone(), read_doc(g, qs, &ignore)))
        .collect();
    let then: Option<BTreeMap<String, Doc>> = match baseline {
        None => None,
        Some(bytes) => Some(
            parse(bytes, "the baseline")?
                .iter()
                .map(|(g, qs)| (g.clone(), read_doc(g, qs, &ignore)))
                .collect(),
        ),
    };

    let mut report = Report::new("corpus");
    report.line(
        "documents",
        match &then {
            Some(t) => format!("{} now, {} at the baseline", now.len(), t.len()),
            None => format!("{} now, no baseline to compare with", now.len()),
        },
    );
    if !ignore.is_empty() {
        report.line(
            "ignored predicates",
            ignore
                .iter()
                .map(|p| short(p))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let out = &mut report.findings;

    for (graph, doc) in &now {
        let before = then.as_ref().and_then(|t| t.get(graph));
        let at = |f: Finding| f.about(graph, &doc.path);
        if let Some(e) = &doc.lift_error {
            let also = before.is_some_and(|b| b.lift_error.is_some());
            out.push(at(Finding::new(
                if also {
                    Severity::Warning
                } else {
                    Severity::Error
                },
                "document-unreadable",
                format!(
                    "the lift cannot read this document{}: {e}",
                    if also { " (nor at the baseline)" } else { "" }
                ),
            )));
            // Nothing else about it can be compared.
            continue;
        }
        if let Some(e) = &doc.frontmatter_error {
            if before.is_none_or(|b| b.frontmatter_error.is_none()) {
                out.push(at(Finding::new(
                    Severity::Error,
                    "frontmatter-unreadable",
                    format!("the frontmatter no longer parses as YAML: {e}"),
                )));
            }
        }
        if let Some(expects) = expects {
            for key in &doc.keys {
                if expects.knows_key(key) {
                    continue;
                }
                let is_new = before.is_some_and(|b| !b.keys.contains(key));
                out.push(at(if is_new {
                    Finding::new(
                        Severity::Warning,
                        "key-unknown",
                        format!(
                            "frontmatter key `{key}` is new since the baseline, and the mapping \
                             does not read it"
                        ),
                    )
                } else {
                    Finding::new(
                        Severity::Info,
                        "key-unknown",
                        format!("frontmatter key `{key}` is not read by the mapping"),
                    )
                }));
            }
        }
        let Some(before) = before else {
            if then.is_some() {
                out.push(at(Finding::new(
                    Severity::Info,
                    "document-new",
                    "a document the baseline does not have",
                )));
            }
            continue;
        };
        if let Some(expects) = expects {
            for key in before.keys.difference(&doc.keys) {
                if expects.knows_key(key) {
                    out.push(at(Finding::new(
                        Severity::Warning,
                        "key-gone",
                        format!("frontmatter key `{key}`, which the mapping reads, is gone"),
                    )));
                }
            }
            headings(expects, before, doc, &mut |f| out.push(at(f)));
        }
        citations(before, doc, &mut |f| out.push(at(f)));
        records(before, doc, &mut |f| out.push(at(f)));
    }
    if let Some(then) = &then {
        for (graph, doc) in then {
            if !now.contains_key(graph) {
                out.push(
                    Finding::new(
                        Severity::Warning,
                        "document-gone",
                        "a document the baseline has is not in the corpus now",
                    )
                    .about(graph, &doc.path),
                );
            }
        }
    }

    if let Some(shapes) = shapes {
        let union = |docs: &BTreeMap<String, Doc>| -> Vec<Triple> {
            docs.values()
                .flat_map(|d| d.triples.iter().cloned())
                .collect()
        };
        let found = crate::shape::validate(shapes, &union(&now));
        let known: BTreeSet<String> = match &then {
            Some(t) => crate::shape::validate(shapes, &union(t))
                .into_iter()
                .map(|v| v.message)
                .collect(),
            None => BTreeSet::new(),
        };
        for v in found {
            // Filed under the first document (by path) that types the focus node.
            let home = now.values().filter(|d| {
                d.triples.iter().any(|t| {
                    t.predicate.as_str() == RDF_TYPE
                        && matches!(&t.subject, NamedOrBlankNode::NamedNode(n) if n.as_str() == v.focus)
                })
            });
            let home = home.min_by(|a, b| a.path.cmp(&b.path));
            let f = if known.contains(&v.message) {
                Finding::new(
                    Severity::Warning,
                    "shape-violation",
                    format!("{} (also at the baseline)", v.message),
                )
            } else {
                Finding::new(Severity::Error, "shape-violation", v.message)
            };
            out.push(match home {
                Some(d) => f.about(&d.graph, &d.path),
                None => f,
            });
        }
    }
    report.sort();
    Ok(report)
}

fn headings(expects: &Expectations, before: &Doc, now: &Doc, push: &mut dyn FnMut(Finding)) {
    let find = |doc: &Doc, text: &str, fold: bool| -> Option<Heading> {
        doc.headings
            .iter()
            .find(|h| {
                if fold {
                    h.text.to_lowercase() == text.to_lowercase()
                } else {
                    h.text == text
                }
            })
            .cloned()
    };
    let under = |p: &Option<String>| match p {
        Some(p) => format!("under {p:?}"),
        None => "at the top".to_string(),
    };
    for (text, fold) in expects.headings() {
        match (find(before, text, fold), find(now, text, fold)) {
            (Some(b), None) => push(Finding::new(
                Severity::Error,
                "heading-vanished",
                format!(
                    "heading {text:?} is gone, and the mapping reads it (it was a level-{} \
                     heading {})",
                    b.level,
                    under(&b.parent)
                ),
            )),
            (Some(b), Some(n)) if b.level != n.level || b.parent != n.parent => {
                let mut what = Vec::new();
                if b.level != n.level {
                    what.push(format!("level {} → {}", b.level, n.level));
                }
                if b.parent != n.parent {
                    what.push(format!("{} → {}", under(&b.parent), under(&n.parent)));
                }
                push(Finding::new(
                    Severity::Warning,
                    "heading-moved",
                    format!(
                        "heading {text:?}, which the mapping reads, moved: {}",
                        what.join("; ")
                    ),
                ))
            }
            _ => {}
        }
    }
    for level in expects.heading_levels() {
        let has = |d: &Doc| d.headings.iter().any(|h| h.level == level);
        if has(before) && !has(now) {
            push(Finding::new(
                Severity::Error,
                "heading-vanished",
                format!("there is no level-{level} heading any more, and the mapping reads one"),
            ));
        }
    }
}

fn citations(before: &Doc, now: &Doc, push: &mut dyn FnMut(Finding)) {
    let keys: BTreeSet<&(String, String)> =
        before.matches.keys().chain(now.matches.keys()).collect();
    for key in keys {
        let b = before.matches.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let n = now.matches.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let (name, value) = key;
        if b.len() > n.len() {
            let (line, text) = &b[0];
            let times = if b.len() - n.len() > 1 {
                format!(" ({} fewer times)", b.len() - n.len())
            } else {
                String::new()
            };
            push(Finding::new(
                Severity::Error,
                "citation-lost",
                format!(
                    "citation `{name}` {value:?} is no longer recognized{times}; at the \
                     baseline it read {text:?} on line {line}"
                ),
            ));
        } else if n.len() > b.len() {
            let (line, text) = &n[0];
            push(Finding::new(
                Severity::Info,
                "citation-gained",
                format!("citation `{name}` {value:?} is newly recognized: {text:?} on line {line}"),
            ));
        }
    }
}

fn records(before: &Doc, now: &Doc, push: &mut dyn FnMut(Finding)) {
    let subjects: BTreeSet<&String> = before.produced.keys().chain(now.produced.keys()).collect();
    for s in subjects {
        match (before.produced.get(s), now.produced.get(s)) {
            (Some(b), None) => push(Finding::new(
                Severity::Error,
                "record-lost",
                format!(
                    "{s} is no longer produced (it had {} at the baseline)",
                    count(b.values().map(Vec::len).sum(), "triple")
                ),
            )),
            (None, Some(n)) => push(Finding::new(
                Severity::Info,
                "record-gained",
                format!(
                    "{s} is newly produced ({})",
                    count(n.values().map(Vec::len).sum(), "triple")
                ),
            )),
            (Some(b), Some(n)) => {
                let predicates: BTreeSet<&String> = b.keys().chain(n.keys()).collect();
                for p in predicates {
                    let empty = Vec::new();
                    let bo = b.get(p).unwrap_or(&empty);
                    let no = n.get(p).unwrap_or(&empty);
                    let lost = minus(bo, no);
                    let gained = minus(no, bo);
                    let p = short(p);
                    // Values replaced by others: a change, whatever the counts, since which
                    // old value became which new one is not knowable from the two sets.
                    if !lost.is_empty() && !gained.is_empty() {
                        push(Finding::new(
                            Severity::Warning,
                            "field-changed",
                            format!("{s} {p}: {} → {}", lost.join(", "), gained.join(", ")),
                        ));
                        continue;
                    }
                    for old in lost {
                        push(Finding::new(
                            Severity::Error,
                            "field-lost",
                            format!("{s} {p} {old} is no longer produced"),
                        ));
                    }
                    for new in gained {
                        push(Finding::new(
                            Severity::Info,
                            "field-gained",
                            format!("{s} {p} {new} is newly produced"),
                        ));
                    }
                }
            }
            (None, None) => {}
        }
    }
}
