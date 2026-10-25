//! The corpus and the linter as resources.
//!
//! - `urn:markdown:lint:snapshot` lifts a LIST of documents (newline-separated IRIs, the
//!   map convention) through `urn:markdown:lift`, one sub-request each, and answers the
//!   union reduced to what the corpus check compares: one named graph per document, sorted
//!   N-Quads. Saved, it is the BASELINE a later corpus check compares with.
//! - `urn:markdown:lint:mapping`, `urn:markdown:lint:expects` and `urn:markdown:lint:corpus`
//!   answer the three checks in [`crate::lint`], as text (the default) or Turtle.
//!
//! Every document, mapping and baseline is read through the kernel, under the caller's
//! capability, so each answer keeps their golden threads: edit a document or the mapping
//! and the next check recomputes, and nothing else does.
//!
//! ★ BOUNDED. A corpus is caller input of every kind at once, so each dimension has a
//! stated bound, and past one the request is REFUSED, naming the bound; it is never
//! answered for part of the corpus. Count and size are [`MAX_DOCUMENTS`],
//! [`MAX_DOCUMENT_BYTES`], [`MAX_CORPUS_BYTES`] and [`MAX_SNAPSHOT_BYTES`]; time is the
//! caller's budget, the one a lift runs under, checked before each document. Depth needs
//! no bound of its own here: the lift reads frontmatter without recursing and runs every
//! construct on a thread sized for it, and snapshots are N-Quads, which do not nest.

use async_trait::async_trait;
use ikigai_core::{
    ArgRef, ArgSpec, Description, Endpoint, Error, Invocation, Iri, ReprType, Representation,
    Request, Result, Verb,
};

use crate::lint::{self, Report};
use crate::sparql::{Allowance, Clock};
use crate::vocab::{md, RDF_TYPE};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};

use crate::{EvaluationBudget, LIFT_IRI, NQUADS};

/// The snapshot's IRI.
pub const SNAPSHOT_IRI: &str = "urn:markdown:lint:snapshot";
/// The snapshot's description id.
pub const SNAPSHOT_ID: &str = "markdown-lint-snapshot";
/// The mapping check's IRI.
pub const LINT_MAPPING_IRI: &str = "urn:markdown:lint:mapping";
/// The mapping check's description id.
pub const LINT_MAPPING_ID: &str = "markdown-lint-mapping";
/// The "what the mapping expects" report's IRI.
pub const LINT_EXPECTS_IRI: &str = "urn:markdown:lint:expects";
/// The "what the mapping expects" report's description id.
pub const LINT_EXPECTS_ID: &str = "markdown-lint-expects";
/// The corpus check's IRI.
pub const LINT_CORPUS_IRI: &str = "urn:markdown:lint:corpus";
/// The corpus check's description id.
pub const LINT_CORPUS_ID: &str = "markdown-lint-corpus";

/// The most documents one corpus may list.
pub const MAX_DOCUMENTS: usize = 10_000;
/// The largest document a corpus may hold.
pub const MAX_DOCUMENT_BYTES: usize = 4 << 20;
/// The most bytes of documents one corpus may hold, all together.
pub const MAX_CORPUS_BYTES: usize = 64 << 20;
/// The largest snapshot: the one being taken, or a baseline.
pub const MAX_SNAPSHOT_BYTES: usize = 256 << 20;
/// The largest `docs` list, in bytes.
pub const MAX_LIST_BYTES: usize = 4 << 20;

const TEXT: &str = "text/plain";
const TURTLE: &str = "text/turtle";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

fn invalid(name: &str, detail: impl Into<String>) -> Error {
    Error::InvalidArgument {
        name: name.to_string(),
        detail: detail.into(),
    }
}

/// The documents a `docs` argument lists: one IRI per non-blank line, sorted, each once.
fn document_list(text: &str) -> Result<Vec<Iri>> {
    if text.len() > MAX_LIST_BYTES {
        return Err(invalid(
            "docs",
            format!(
                "the list is {} bytes, and a corpus list may be at most {MAX_LIST_BYTES} \
                 (MAX_LIST_BYTES); lint the corpus in parts",
                text.len()
            ),
        ));
    }
    let mut names: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    if names.len() > MAX_DOCUMENTS {
        return Err(invalid(
            "docs",
            format!(
                "the list names {} documents, and a corpus may hold at most {MAX_DOCUMENTS} \
                 (MAX_DOCUMENTS); lint the corpus in parts",
                names.len()
            ),
        ));
    }
    if names.is_empty() {
        return Err(invalid(
            "docs",
            "the list names no documents: give one resource IRI per line",
        ));
    }
    names
        .into_iter()
        .map(|n| {
            Iri::parse(n).map_err(|e| invalid("docs", format!("`{n}` is not a valid IRI: {e}")))
        })
        .collect()
}

/// The output face a lint endpoint was asked for: `true` for Turtle.
fn turtle_face(inv: &Invocation<'_>) -> Result<bool> {
    match inv.inline_str("as").map(str::trim) {
        Err(_) | Ok(TEXT) => Ok(false),
        Ok(TURTLE) => Ok(true),
        Ok(other) => Err(invalid(
            "as",
            format!("expected {TEXT} or {TURTLE}, got {other:?}"),
        )),
    }
}

fn answer(text: String, turtle: bool) -> Representation {
    let media = if turtle { TURTLE } else { TEXT };
    // A function of the arguments and of what they resolved to: the kernel folds in the
    // threads of every resource read on the way.
    Representation::new(ReprType::new(media), text.into_bytes()).cacheable()
}

fn render_report(report: &Report, turtle: bool) -> Result<Representation> {
    let text = if turtle {
        report.to_turtle().map_err(Error::Endpoint)?
    } else {
        report.to_text()
    };
    Ok(answer(text, turtle))
}

/// The `mapping` argument, required, as (Turtle, base IRI).
async fn mapping(inv: &Invocation<'_>) -> Result<(String, String)> {
    let value = inv.inline_str("mapping").map_err(|_| {
        invalid(
            "mapping",
            "name the mapping: an IRI (urn:markdown:mapping:{name} for one in the config \
             home) or its Turtle by value",
        )
    })?;
    crate::mapping_text(inv, value).await
}

fn output(name: &str) -> ArgSpec {
    ArgSpec::new(name)
        .summary("output media type")
        .class(XSD_STRING)
        .one_of([TEXT, TURTLE])
        .default_value(TEXT)
        .optional()
}

fn mapping_input(optional: bool) -> ArgSpec {
    let spec = ArgSpec::new("mapping")
        .summary(
            "the mapping: an IRI (its golden thread is kept; urn:markdown:mapping:{name} names \
             one in the config home) or its Turtle by value",
        )
        .class(XSD_STRING);
    if optional {
        spec.optional()
    } else {
        spec
    }
}

fn docs_input() -> ArgSpec {
    ArgSpec::new("docs")
        .summary(
            "the documents: one resource IRI per line (pipe a list in). Each is read through \
             the kernel and lifted by reference, so its golden thread is kept",
        )
        .class(XSD_STRING)
}

fn root_input() -> ArgSpec {
    ArgSpec::new("root")
        .summary(
            "an IRI prefix every document shares; what follows it is the document's path \
             (md:path), and its graph is urn:markdown:doc:{path}, so snapshots taken on two \
             machines name documents alike. Omitted, a document's path is its IRI",
        )
        .class(XSD_STRING)
        .optional()
}

// --- urn:markdown:lint:snapshot ---------------------------------------------------------

pub(crate) struct SnapshotEndpoint {
    pub budget: EvaluationBudget,
}

impl SnapshotEndpoint {
    /// Lift every listed document; the union, reduced, as sorted N-Quads.
    async fn lift(
        &self,
        inv: &Invocation<'_>,
        docs: &str,
        root: Option<&str>,
        mapping: Option<&str>,
    ) -> Result<Vec<u8>> {
        let list = document_list(docs)?;
        let now = || inv.now();
        let allowance = Allowance::new(&self.budget, inv.capability, Clock::of(&now));
        let (mut read, mut produced) = (0usize, 0usize);
        let mut lines: Vec<String> = Vec::new();
        for (i, iri) in list.iter().enumerate() {
            allowance.check_left(&format!(
                "the snapshot (at document {} of {})",
                i + 1,
                list.len()
            ))?;
            let path = match root {
                None => None,
                Some(root) => match iri.as_str().strip_prefix(root) {
                    Some(rest) if !rest.is_empty() => Some(rest.to_string()),
                    _ => {
                        return Err(invalid(
                            "docs",
                            format!("`{iri}` is not under the root `{root}`"),
                        ))
                    }
                },
            };
            // Read first, to bound it before the lift parses it; the lift's own read of the
            // same name is then the kernel's cached answer when the document is cacheable.
            let bytes = inv.source(iri).await?.bytes.len();
            if bytes > MAX_DOCUMENT_BYTES {
                return Err(invalid(
                    "docs",
                    format!(
                        "`{iri}` is {bytes} bytes, and a corpus document may be at most \
                         {MAX_DOCUMENT_BYTES} (MAX_DOCUMENT_BYTES)"
                    ),
                ));
            }
            read += bytes;
            if read > MAX_CORPUS_BYTES {
                return Err(invalid(
                    "docs",
                    format!(
                        "the documents come to more than {MAX_CORPUS_BYTES} bytes \
                         (MAX_CORPUS_BYTES) by `{iri}`; lint the corpus in parts"
                    ),
                ));
            }
            let mut request = Request::new(
                Verb::Source,
                Iri::parse(LIFT_IRI).expect("LIFT_IRI is an IRI"),
            )
            .with_arg("src", ArgRef::Inline(iri.as_str().as_bytes().to_vec()));
            if let Some(path) = &path {
                request = request.with_arg("path", ArgRef::Inline(path.as_bytes().to_vec()));
            }
            if let Some(m) = mapping {
                request = request.with_arg("mapping", ArgRef::Inline(m.as_bytes().to_vec()));
            }
            let lifted = match inv.issue(request).await {
                Ok(lifted) => lifted,
                // A document the lift cannot read (not UTF-8) is part of the answer, not a
                // refusal of the whole corpus: its graph says so, and the check reports it.
                Err(Error::InvalidArgument { name, detail }) if name == "src" => {
                    for line in unreadable(path.as_deref(), iri, &detail)? {
                        produced += line.len() + 1;
                        lines.push(line);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            // One document's lift is in proportion to the document, which is bounded above;
            // the snapshot keeps only its reduction, and that is what the bound counts.
            let mut quads = Vec::new();
            for quad in RdfParser::from_format(RdfFormat::NQuads).for_slice(&lifted.bytes) {
                quads.push(quad.map_err(|e| {
                    Error::Endpoint(format!("the lift of `{iri}` is not N-Quads: {e}"))
                })?);
            }
            for quad in crate::compare::reduce(quads) {
                let line = format!("{quad} .");
                produced += line.len() + 1;
                lines.push(line);
            }
            if produced > MAX_SNAPSHOT_BYTES {
                return Err(invalid(
                    "docs",
                    format!(
                        "the snapshot comes to more than {MAX_SNAPSHOT_BYTES} bytes \
                         (MAX_SNAPSHOT_BYTES) by `{iri}`; lint the corpus in parts"
                    ),
                ));
            }
        }
        lines.sort();
        lines.dedup();
        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

/// A predicate named `prefix:local` with one of `prefixes` (or a namespace every report
/// writes short) as its IRI; anything else as it was written.
fn expand(token: &str, prefixes: &[(String, String)]) -> String {
    let token = token.trim_start_matches('<').trim_end_matches('>');
    if let Some((p, local)) = token.split_once(':') {
        let ns = prefixes
            .iter()
            .map(|(p, ns)| (p.as_str(), ns.as_str()))
            .chain(crate::analyze::PREFIXES)
            .find(|(prefix, _)| *prefix == p)
            .map(|(_, ns)| ns);
        if let Some(ns) = ns {
            return format!("{ns}{local}");
        }
    }
    token.to_string()
}

/// The snapshot lines of a document the lift could not read: its graph, typed and pathed
/// as a lifted one is, with `md:liftError` saying why.
fn unreadable(path: Option<&str>, iri: &Iri, why: &str) -> Result<Vec<String>> {
    let doc = crate::doc_ref(path, Some(iri.as_str()))?;
    let graph = GraphName::NamedNode(doc.iri.clone());
    let line =
        |p: NamedNode, o: Term| format!("{} .", Quad::new(doc.iri.clone(), p, o, graph.clone()));
    Ok(vec![
        line(NamedNode::new_unchecked(RDF_TYPE), md("Document").into()),
        line(md("path"), Literal::from(doc.path.as_str()).into()),
        line(md("liftError"), Literal::from(why).into()),
    ])
}

#[async_trait]
impl Endpoint for SnapshotEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let docs = inv
            .inline_str("docs")
            .map_err(|_| invalid("docs", "list the documents: one resource IRI per line"))?;
        let root = inv.inline_str("root").ok();
        let mapping = inv.inline_str("mapping").ok();
        let bytes = self.lift(inv, docs, root, mapping).await?;
        Ok(Representation::new(ReprType::new(NQUADS), bytes).cacheable())
    }

    fn name(&self) -> &str {
        SNAPSHOT_ID
    }

    fn describe(&self) -> Description {
        Description::new(SNAPSHOT_ID)
            .title("Markdown corpus snapshot")
            .summary(
                "Lift a list of documents through urn:markdown:lift and answer the union, \
                 reduced to what urn:markdown:lint:corpus compares (everything the mapping \
                 produced; the structure's paths, headings, frontmatter keys and citation \
                 matches): one named graph per document, sorted N-Quads. Saved, it is the \
                 baseline a later check compares with. Bounded in documents, bytes and the \
                 caller's time budget; past a bound it refuses, never answering for part of \
                 the corpus.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(docs_input())
            .input(root_input())
            .input(mapping_input(true))
            .output(NQUADS)
    }
}

// --- urn:markdown:lint:mapping and urn:markdown:lint:expects ----------------------------

pub(crate) struct LintMappingEndpoint;

#[async_trait]
impl Endpoint for LintMappingEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let turtle = turtle_face(inv)?;
        let (text, base) = mapping(inv).await?;
        render_report(&lint::check_mapping(&text, &base), turtle)
    }

    fn name(&self) -> &str {
        LINT_MAPPING_ID
    }

    fn describe(&self) -> Description {
        Description::new(LINT_MAPPING_ID)
            .title("Markdown mapping check")
            .summary(
                "Check a mapping on its own: every lift-profile pattern compiles, every \
                 md:construct parses and is a CONSTRUCT within the lift's bounds, and what the \
                 constructs write conforms to the shape the mapping states with md:shape (or \
                 the report says no shape is stated). A mapping that does not load is reported, \
                 not refused.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(mapping_input(false))
            .input(output("as"))
            .output(TEXT)
            .output(TURTLE)
    }
}

pub(crate) struct LintExpectsEndpoint;

#[async_trait]
impl Endpoint for LintExpectsEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let turtle = turtle_face(inv)?;
        let (text, base) = mapping(inv).await?;
        let expects = lint::expectations(&text, &base).map_err(|e| invalid("mapping", e))?;
        let body = if turtle {
            expects.to_turtle().map_err(Error::Endpoint)?
        } else {
            expects.to_text()
        };
        Ok(answer(body, turtle))
    }

    fn name(&self) -> &str {
        LINT_EXPECTS_ID
    }

    fn describe(&self) -> Description {
        Description::new(LINT_EXPECTS_ID)
            .title("What a markdown mapping expects")
            .summary(
                "What a mapping depends on in a document, for the author of the documents: \
                 the paths, frontmatter keys, headings, heading levels and citation forms it \
                 reads, and every other literal its constructs compare a document's text with.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(mapping_input(false))
            .input(output("as"))
            .output(TEXT)
            .output(TURTLE)
    }
}

// --- urn:markdown:lint:corpus -----------------------------------------------------------

pub(crate) struct LintCorpusEndpoint;

#[async_trait]
impl Endpoint for LintCorpusEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let turtle = turtle_face(inv)?;
        let docs = inv
            .inline_str("docs")
            .map_err(|_| invalid("docs", "list the documents: one resource IRI per line"))?;
        let mapping_arg = inv
            .inline_str("mapping")
            .map_err(|_| invalid("mapping", "name the mapping the documents are lifted under"))?;
        let (mapping_text, base) = mapping(inv).await?;
        let root = inv.inline_str("root").ok();
        let ignore: Vec<String> = inv
            .inline_str("ignore")
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();

        let header = |report: &mut Report, baseline: &str| {
            report
                .about
                .insert(0, ("mapping".to_string(), base.clone()));
            report
                .about
                .insert(1, ("baseline".to_string(), baseline.to_string()));
        };

        // A mapping that does not load is the answer, not a refusal.
        let read = match lint::read(&mapping_text, &base) {
            Ok(read)
                if read.raw.problems.is_empty()
                    && read.constructs.iter().all(|c| c.problem.is_none()) =>
            {
                read
            }
            Ok(_) | Err(_) => {
                let mut report = lint::check_mapping(&mapping_text, &base);
                report.check = "corpus".to_string();
                report
                    .findings
                    .retain(|f| f.severity == lint::Severity::Error);
                report.findings.push(lint::Finding::new(
                    lint::Severity::Error,
                    "mapping-unusable",
                    "the corpus was not lifted: the mapping does not load (see \
                     urn:markdown:lint:mapping)",
                ));
                report.about.clear();
                header(&mut report, "not read");
                report.sort();
                return render_report(&report, turtle);
            }
        };

        // The corpus NOW is the snapshot resource itself, sourced through the kernel: one
        // cached answer per (documents, mapping), on the threads of both, which a saved
        // baseline taken by the same call compares with byte for byte.
        let mut request = Request::new(
            Verb::Source,
            Iri::parse(SNAPSHOT_IRI).expect("SNAPSHOT_IRI is an IRI"),
        )
        .with_arg("docs", ArgRef::Inline(docs.as_bytes().to_vec()))
        .with_arg("mapping", ArgRef::Inline(mapping_arg.as_bytes().to_vec()));
        if let Some(root) = root {
            request = request.with_arg("root", ArgRef::Inline(root.as_bytes().to_vec()));
        }
        let current = match inv.issue(request).await {
            Ok(snapshot) => snapshot.bytes,
            Err(Error::InvalidArgument { name, detail }) if name == "mapping" => {
                let mut report = Report::new("corpus");
                header(&mut report, "not read");
                report.findings.push(lint::Finding::new(
                    lint::Severity::Error,
                    "lift-refused",
                    format!("the lift refuses this mapping: {detail}"),
                ));
                return render_report(&report, turtle);
            }
            Err(e) => return Err(e),
        };

        let (baseline, baseline_label) = match inv.inline_str("baseline") {
            Err(_) => (None, "none".to_string()),
            Ok(b) if crate::is_reference(b) => {
                let iri = crate::parse_iri("baseline", b)?;
                (Some(inv.source(&iri).await?.bytes), b.to_string())
            }
            Ok(b) => (Some(b.as_bytes().to_vec()), "given by value".to_string()),
        };
        if let Some(b) = &baseline {
            if b.len() > MAX_SNAPSHOT_BYTES {
                return Err(invalid(
                    "baseline",
                    format!(
                        "the baseline is {} bytes, and a snapshot may be at most \
                         {MAX_SNAPSHOT_BYTES} (MAX_SNAPSHOT_BYTES)",
                        b.len()
                    ),
                ));
            }
        }

        // `ex:line` in the mapping's own prefixes, or a full IRI.
        let ignore: Vec<String> = ignore
            .iter()
            .map(|token| expand(token, &read.raw.prefixes))
            .collect();
        let expects = lint::expectations_of(&read, &base);
        let mut report = crate::compare::compare(
            Some(&expects),
            read.shapes.as_ref(),
            &current,
            baseline.as_deref(),
            &ignore,
        )
        .map_err(|e| {
            if e.starts_with("the baseline") {
                invalid("baseline", e)
            } else {
                Error::Endpoint(e)
            }
        })?;
        header(&mut report, &baseline_label);
        report.abbreviate(&read.raw.prefixes);
        report.sort();
        render_report(&report, turtle)
    }

    fn name(&self) -> &str {
        LINT_CORPUS_ID
    }

    fn describe(&self) -> Description {
        Description::new(LINT_CORPUS_ID)
            .title("Markdown corpus check")
            .summary(
                "Lift a list of documents under a mapping and compare each with a baseline \
                 snapshot (a saved urn:markdown:lint:snapshot answer): records and fields lost, \
                 gained or changed, citations no longer recognized, headings the mapping reads \
                 that vanished or moved, frontmatter keys the mapping does not read, and values \
                 that break the shape the mapping states. Sorted and deterministic.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(docs_input())
            .input(mapping_input(false))
            .input(root_input())
            .input(
                ArgSpec::new("baseline")
                    .summary(
                        "the snapshot to compare with: an IRI (a saved \
                         urn:markdown:lint:snapshot answer; its golden thread is kept) or the N-Quads by value. Omitted, \
                         only what needs no baseline is checked",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(
                ArgSpec::new("ignore")
                    .summary(
                        "predicates, space-separated, left out of the record comparison (a \
                         line number the mapping copies onto every record moves whenever a \
                         line is added above it): full IRIs, or prefixed names in the \
                         mapping's own prefixes",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(output("as"))
            .output(TEXT)
            .output(TURTLE)
    }
}
