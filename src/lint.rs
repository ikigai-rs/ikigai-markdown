//! The mapping linter: three checks a spec author runs when a document format changes.
//!
//! - [`check_mapping`] (`urn:markdown:lint:mapping`): the mapping on its own. Every
//!   profile pattern compiles as a Rust regex, every construct parses and is a CONSTRUCT
//!   within the lift's bounds, and what the constructs write is checked against the shape
//!   the mapping states with `md:shape`, or the report says that no shape is stated.
//! - [`expectations`] (`urn:markdown:lint:expects`): what the mapping DEPENDS ON in a
//!   document: the paths, frontmatter keys, headings, heading levels and citation forms
//!   it reads, and every other literal its constructs compare a document's text with. The
//!   list of things a spec author must not break.
//! - The corpus check (`urn:markdown:lint:corpus`, over `urn:markdown:lint:snapshot`): a
//!   corpus lifted now against a baseline snapshot, per document: records and fields lost,
//!   gained or changed, citations no longer recognized, headings the mapping reads that
//!   vanished or moved, frontmatter keys it does not read, and values that break the shape.
//!
//! Every check is a pure function of its inputs and answers with a [`Report`] or an
//! [`Expectations`], each with a text face for people and a Turtle face for queries. A
//! mapping that does not load is a FINDING, not a refusal: "this input is wrong" is the
//! answer a linter exists to give, and a refusal would never be cached.

use std::collections::{BTreeMap, BTreeSet};

use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{Literal, NamedNode, Triple};

use crate::analyze::{short, ConstructFacts};
pub use crate::analyze::{Expect, ExpectKind, How, Produces};
use crate::mapping::RawMapping;
use crate::shape::Shapes;
use crate::vocab::{md, MD, RDF_TYPE, XSD_INTEGER};

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something the mapping relied on no longer holds, or the lift would refuse.
    Error,
    /// Worth a look: it may or may not break what is built on the mapping.
    Warning,
    /// Information: a change that cannot break anything, or something not checked.
    Info,
}

impl Severity {
    /// Its name on the wire (`md:severity`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One thing a check found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Finding {
    /// The document it is about, by its graph IRI; `None` for the mapping or the corpus.
    pub document: Option<String>,
    /// The document's path as the lift recorded it (`md:path`), for display.
    pub path: Option<String>,
    /// How much it matters.
    pub severity: Severity,
    /// A stable kebab-case code, e.g. `heading-vanished`.
    pub code: String,
    /// What was found, in one line.
    pub message: String,
}

impl Finding {
    /// A finding about the mapping or the corpus. The message is ONE line: a parser's
    /// multi-line error is joined, so the text face stays one finding per line.
    pub(crate) fn new(severity: Severity, code: &str, message: impl Into<String>) -> Finding {
        let message: String = message.into();
        let message = if message.contains('\n') {
            message.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            message
        };
        Finding {
            document: None,
            path: None,
            severity,
            code: code.to_string(),
            message,
        }
    }

    pub(crate) fn about(mut self, document: &str, path: &str) -> Finding {
        self.document = Some(document.to_string());
        self.path = Some(path.to_string());
        self
    }
}

/// What a check reports: a few lines saying what was checked, then the findings, sorted
/// by document, severity, code and message, so the same inputs give the same bytes.
#[derive(Debug, Clone)]
pub struct Report {
    /// Which check: `mapping` or `corpus`.
    pub check: String,
    /// What was checked, as (label, value) lines.
    pub about: Vec<(String, String)>,
    /// What was found.
    pub findings: Vec<Finding>,
}

impl Report {
    pub(crate) fn new(check: &str) -> Report {
        Report {
            check: check.to_string(),
            about: Vec::new(),
            findings: Vec::new(),
        }
    }

    pub(crate) fn line(&mut self, label: &str, value: impl Into<String>) {
        self.about.push((label.to_string(), value.into()));
    }

    /// Write every `<iri>` in the messages as `prefix:local` with the mapping's own
    /// `@prefix` declarations, so a report reads in the mapping author's words.
    pub(crate) fn abbreviate(&mut self, prefixes: &[(String, String)]) {
        for f in &mut self.findings {
            f.message = abbreviate(&f.message, prefixes);
        }
    }

    pub(crate) fn sort(&mut self) {
        self.findings.sort_by(|a, b| {
            (&a.path, &a.document, a.severity, &a.code, &a.message).cmp(&(
                &b.path,
                &b.document,
                b.severity,
                &b.code,
                &b.message,
            ))
        });
        self.findings.dedup();
    }

    /// How many findings have `severity`.
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }

    /// The text face: what was checked, a tally, then the findings grouped by document.
    ///
    /// ```
    /// let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
    /// <urn:ex:m> a md:Mapping ; md:citation <urn:ex:m:c> .
    /// <urn:ex:m:c> md:name "issue" ; md:pattern "#([0-9]+" ."##;
    /// let report = ikigai_markdown::lint::check_mapping(ttl, "urn:ex:m");
    /// let text = report.to_text();
    /// assert!(text.starts_with("markdown lint: mapping\n"), "{text}");
    /// assert!(text.contains("  result: 1 error, 0 warnings, 1 note\n"), "{text}");
    /// assert!(text.contains("\n(the mapping)\n  error    profile-invalid  md:citation `issue`: pattern is not a regex"), "{text}");
    /// ```
    pub fn to_text(&self) -> String {
        let mut out = format!("markdown lint: {}\n", self.check);
        for (label, value) in &self.about {
            out.push_str(&format!("  {label}: {value}\n"));
        }
        out.push_str(&format!("  result: {}\n", self.tally()));
        let mut current: Option<Option<&str>> = None;
        for f in &self.findings {
            let group = f.path.as_deref();
            if current != Some(group) {
                current = Some(group);
                out.push('\n');
                match group {
                    Some(path) => out.push_str(&format!("{path}\n")),
                    None => out.push_str(&format!("(the {})\n", self.check)),
                }
            }
            out.push_str(&format!(
                "  {:<8} {}  {}\n",
                f.severity.as_str(),
                f.code,
                f.message
            ));
        }
        out
    }

    fn tally(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        format!(
            "{}, {}, {}",
            plural(self.count(Severity::Error), "error", "errors"),
            plural(self.count(Severity::Warning), "warning", "warnings"),
            plural(self.count(Severity::Info), "note", "notes"),
        )
    }

    /// The Turtle face: one `md:LintReport` and one `md:Finding` per finding, every node
    /// named by a digest of what it says, so two runs that find the same thing name it
    /// the same way and reports union cleanly. No blank nodes.
    pub fn to_turtle(&self) -> Result<String, String> {
        let report = node(&format!(
            "urn:markdown:lint:report:{}",
            digest(&self.to_text())
        ));
        let mut triples = vec![
            Triple::new(report.clone(), rdf_type(), md("LintReport")),
            Triple::new(
                report.clone(),
                md("check"),
                Literal::from(self.check.as_str()),
            ),
        ];
        for (p, severity) in [
            ("errors", Severity::Error),
            ("warnings", Severity::Warning),
            ("notes", Severity::Info),
        ] {
            triples.push(Triple::new(
                report.clone(),
                md(p),
                integer(self.count(severity)),
            ));
        }
        for f in &self.findings {
            let key = format!(
                "{}\n{}\n{}\n{}",
                self.check,
                f.document.as_deref().unwrap_or(""),
                f.code,
                f.message
            );
            let n = node(&format!("urn:markdown:lint:finding:{}", digest(&key)));
            triples.push(Triple::new(report.clone(), md("finding"), n.clone()));
            triples.push(Triple::new(n.clone(), rdf_type(), md("Finding")));
            triples.push(Triple::new(
                n.clone(),
                md("severity"),
                Literal::from(f.severity.as_str()),
            ));
            triples.push(Triple::new(
                n.clone(),
                md("code"),
                Literal::from(f.code.as_str()),
            ));
            triples.push(Triple::new(
                n.clone(),
                md("message"),
                Literal::from(f.message.as_str()),
            ));
            if let Some(d) = f.document.as_deref().and_then(|d| NamedNode::new(d).ok()) {
                triples.push(Triple::new(n.clone(), md("document"), d));
            }
            if let Some(p) = &f.path {
                triples.push(Triple::new(
                    n,
                    md("documentPath"),
                    Literal::from(p.as_str()),
                ));
            }
        }
        turtle(triples)
    }
}

/// `text` with each `<iri>` under one of `prefixes`, or a namespace every mapping uses
/// (`md:`, `xsd:`, `rdf:`, `rdfs:`, `sh:`), written `prefix:local`; anything else in angle
/// brackets is left as it is.
///
/// ```text
/// <http://example.org/decisions#title> with dr: = http://example.org/decisions#  → dr:title
/// ```
pub(crate) fn abbreviate(text: &str, prefixes: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let iri = after
            .find('>')
            .map(|end| &after[..end])
            .filter(|iri| !iri.contains(|c: char| c.is_whitespace() || c == '<'));
        let short = iri.and_then(|iri| {
            prefixes
                .iter()
                .map(|(p, ns)| (p.as_str(), ns.as_str()))
                .chain(crate::analyze::PREFIXES)
                .filter(|(p, ns)| !p.is_empty() && !ns.is_empty())
                .filter_map(|(p, ns)| iri.strip_prefix(ns).map(|local| (p, ns, local)))
                .filter(|(_, _, local)| {
                    !local.is_empty()
                        && !local.ends_with('.')
                        && local
                            .chars()
                            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
                })
                .max_by_key(|(_, ns, _)| ns.len())
                .map(|(p, _, local)| format!("{p}:{local}"))
        });
        match (iri, short) {
            (Some(iri), Some(short)) => {
                out.push_str(&short);
                rest = &after[iri.len() + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn node(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn rdf_type() -> NamedNode {
    NamedNode::new_unchecked(RDF_TYPE)
}

fn integer(n: usize) -> Literal {
    Literal::new_typed_literal(n.to_string(), NamedNode::new_unchecked(XSD_INTEGER))
}

/// The first 32 hex digits of the BLAKE3 digest of `text`, for naming a node.
fn digest(text: &str) -> String {
    let id = ikigai_core::ContentId::of(text.as_bytes()).to_string();
    id.trim_start_matches("b3:").chars().take(32).collect()
}

/// Serialize sorted, de-duplicated triples as Turtle with the `md:` prefix.
fn turtle(mut triples: Vec<Triple>) -> Result<String, String> {
    triples.sort_by_cached_key(|t| t.to_string());
    triples.dedup();
    let mut writer = RdfSerializer::from_format(RdfFormat::Turtle)
        .with_prefix("md", MD)
        .map_err(|e| e.to_string())?
        .for_writer(Vec::new());
    for t in &triples {
        writer.serialize_triple(t).map_err(|e| e.to_string())?;
    }
    let bytes = writer.finish().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

// --- a mapping, read for linting ------------------------------------------------------

/// A mapping as the linter reads it: leniently, every construct analyzed.
pub(crate) struct Read {
    pub raw: RawMapping,
    pub constructs: Vec<ConstructFacts>,
    pub shapes: Option<Shapes>,
}

/// Read `turtle` for linting, or say why there is nothing to read.
pub(crate) fn read(turtle: &str, base: &str) -> Result<Read, String> {
    let raw = crate::mapping::read_raw(turtle, base)?;
    let constructs = raw
        .constructs
        .iter()
        .enumerate()
        .map(|(i, text)| crate::analyze::construct(text, i + 1))
        .collect();
    let shapes = crate::shape::read(&raw);
    Ok(Read {
        raw,
        constructs,
        shapes,
    })
}

// --- check 1: the mapping on its own -------------------------------------------------

/// Check a mapping on its own (`base` resolves its relative IRIs; pass the IRI it was
/// read from). Never fails: a mapping that cannot be read is reported as an error.
///
/// ```
/// let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
/// <urn:ex:m> a md:Mapping ; md:construct "SELECT * WHERE { ?s ?p ?o }" ."##;
/// let report = ikigai_markdown::lint::check_mapping(ttl, "urn:ex:m");
/// assert!(report.findings.iter().any(|f| f.code == "construct-invalid"
///     && f.message == "construct 1 is not a CONSTRUCT query"));
/// assert!(report.findings.iter().any(|f| f.code == "no-shape"));
/// ```
pub fn check_mapping(turtle: &str, base: &str) -> Report {
    let mut report = Report::new("mapping");
    report.line("mapping", base);
    let read = match read(turtle, base) {
        Ok(read) => read,
        Err(why) => {
            report
                .findings
                .push(Finding::new(Severity::Error, "mapping-unreadable", why));
            return report;
        }
    };
    let profile = &read.raw.profile;
    report.line(
        "profile",
        format!(
            "{} split(s), {} citation pattern(s)",
            profile.splits.len(),
            profile.citations.len()
        ),
    );
    report.line("constructs", read.constructs.len().to_string());
    report.findings.extend(mapping_findings(&read));
    report.abbreviate(&read.raw.prefixes);
    report.sort();
    report
}

fn mapping_findings(read: &Read) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut push = |s: Severity, code: &str, m: String| out.push(Finding::new(s, code, m));
    for problem in &read.raw.problems {
        let code = if problem.starts_with("md:construct") {
            "construct-invalid"
        } else {
            "profile-invalid"
        };
        push(Severity::Error, code, problem.clone());
    }

    let profile = &read.raw.profile;
    for c in &profile.citations {
        if c.pattern.is_match("") {
            push(
                Severity::Warning,
                "citation-matches-empty",
                format!(
                    "md:citation `{}`: the pattern can match the empty string, and the lift \
                     drops every empty match",
                    c.name
                ),
            );
        }
        let has_value_group =
            c.pattern.capture_names().flatten().any(|n| n == "v") || c.pattern.captures_len() > 1;
        if !has_value_group {
            push(
                Severity::Info,
                "citation-whole-match",
                format!(
                    "md:citation `{}`: no group `v` and no group 1, so md:value is the whole match",
                    c.name
                ),
            );
        }
    }

    let mut names_read: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut keys_read: BTreeSet<String> = BTreeSet::new();
    let reads_any = read.constructs.iter().any(|c| c.reads_any_citation);
    for c in &read.constructs {
        if let Some(problem) = &c.problem {
            push(Severity::Error, "construct-invalid", problem.clone());
            continue;
        }
        if c.produces.blank_node {
            push(
                Severity::Error,
                "construct-blank-node",
                format!(
                    "construct {} mints a blank node in its template, which the lift refuses; \
                     build IRIs with IRI(CONCAT(…))",
                    c.index
                ),
            );
        }
        for p in c.produces.predicates.iter().filter(|p| p.starts_with(MD)) {
            push(
                Severity::Warning,
                "construct-writes-structure",
                format!(
                    "construct {} writes {}, a structural term: the corpus check reads it as \
                     structure, not as what the mapping produced",
                    c.index,
                    short(p)
                ),
            );
        }
        for e in &c.expects {
            match e.kind {
                ExpectKind::CitationName => {
                    names_read
                        .entry(e.value.clone())
                        .or_default()
                        .insert(c.index);
                }
                ExpectKind::FrontmatterKey => {
                    keys_read.insert(e.value.to_lowercase());
                }
                _ => {}
            }
        }
    }
    for (name, constructs) in &names_read {
        if !profile.citations.iter().any(|c| &c.name == name) {
            push(
                Severity::Warning,
                "citation-undefined",
                format!(
                    "{} compare(s) md:matchedBy with `{name}`, which the profile defines no \
                     md:citation for, so it never matches",
                    list(constructs)
                ),
            );
        }
    }
    if !reads_any {
        for c in &profile.citations {
            if !names_read.contains_key(&c.name) {
                push(
                    Severity::Info,
                    "citation-unread",
                    format!(
                        "md:citation `{}` is matched by the lift but no construct reads it",
                        c.name
                    ),
                );
            }
        }
    }
    for s in &profile.splits {
        if !keys_read.contains(&s.key.to_lowercase()) {
            push(
                Severity::Info,
                "split-unread",
                format!(
                    "md:split `{}` is split by the lift but no construct names that key",
                    s.key
                ),
            );
        }
    }

    match &read.shapes {
        None => push(
            Severity::Info,
            "no-shape",
            "no shape stated: the mapping has no md:shape, so what its constructs write is \
             not checked"
                .to_string(),
        ),
        Some(shapes) => out.extend(shape_findings(read, shapes)),
    }
    out
}

fn shape_findings(read: &Read, shapes: &Shapes) -> Vec<Finding> {
    let mut out = Vec::new();
    for u in &shapes.unchecked {
        out.push(Finding::new(
            Severity::Warning,
            "shape-unchecked",
            u.clone(),
        ));
    }
    let declared = shapes.declared_predicates();
    let classes = shapes.declared_classes();
    let mut written: BTreeSet<&str> = BTreeSet::new();
    let mut written_classes: BTreeSet<&str> = BTreeSet::new();
    let mut computed = false;
    for c in read.constructs.iter().filter(|c| c.problem.is_none()) {
        computed |= c.produces.computed_predicate;
        written.extend(c.produces.predicates.iter().map(String::as_str));
        written_classes.extend(c.produces.classes.iter().map(String::as_str));
        for p in &c.produces.predicates {
            if p != RDF_TYPE && !p.starts_with(MD) && !declared.contains(p.as_str()) {
                out.push(Finding::new(
                    Severity::Warning,
                    "undeclared-predicate",
                    format!(
                        "construct {} writes {}, which no shape declares",
                        c.index,
                        short(p)
                    ),
                ));
            }
        }
        for k in &c.produces.classes {
            if !classes.contains(k.as_str()) {
                out.push(Finding::new(
                    Severity::Warning,
                    "undeclared-class",
                    format!(
                        "construct {} types nodes {}, which no shape targets",
                        c.index,
                        short(k)
                    ),
                ));
            }
        }
        if c.produces.computed_predicate || c.produces.computed_class {
            out.push(Finding::new(
                Severity::Info,
                "computed-term",
                format!(
                    "construct {} chooses a predicate or class at run time; it is not checked \
                     against the shape",
                    c.index
                ),
            ));
        }
    }
    for shape in &shapes.shapes {
        for target in &shape.targets {
            if !written_classes.contains(target.as_str()) && !computed {
                out.push(Finding::new(
                    Severity::Warning,
                    "shape-unproduced",
                    format!(
                        "{} targets {}, and no construct types a node so",
                        shape.name,
                        short(target)
                    ),
                ));
            }
        }
        for p in &shape.properties {
            if p.min.unwrap_or(0) > 0 && !written.contains(p.path.as_str()) && !computed {
                out.push(Finding::new(
                    Severity::Warning,
                    "shape-unproduced",
                    format!(
                        "{} asks for {} (sh:minCount {}), and no construct writes it",
                        shape.name,
                        short(&p.path),
                        p.min.unwrap_or(0)
                    ),
                ));
            }
        }
    }
    out
}

/// `construct 1, 3` / `construct 2`.
fn list(constructs: &BTreeSet<usize>) -> String {
    let n: Vec<String> = constructs.iter().map(usize::to_string).collect();
    format!("construct {}", n.join(", "))
}

// --- check 3: what the mapping expects ------------------------------------------------

/// What a mapping depends on in a document, for a spec author to read.
#[derive(Debug, Clone, Default)]
pub struct Expectations {
    /// The mapping, as it was named.
    pub mapping: String,
    /// Frontmatter keys split into tokens: (key, delimiter regex).
    pub splits: Vec<(String, String)>,
    /// Citation patterns: (name, Rust regex).
    pub citations: Vec<(String, String)>,
    /// Every literal the constructs compare a document's text with, and which constructs.
    pub expects: BTreeMap<Expect, BTreeSet<usize>>,
    /// Predicates the constructs write.
    pub writes: BTreeSet<String>,
    /// Classes the constructs type nodes with.
    pub types: BTreeSet<String>,
    /// Constructs that could not be read, and why; what they depend on is not listed.
    pub unreadable: Vec<String>,
    /// The shapes stated, by name, or empty.
    pub shapes: Vec<String>,
    /// The mapping's `@prefix` declarations, for display.
    pub prefixes: Vec<(String, String)>,
    /// Constructs that read `md:matchedBy` into a variable, so read citations of any name.
    pub any_citation: BTreeSet<usize>,
}

/// What `turtle` (a mapping, read against `base`) depends on in a document.
///
/// ```
/// use ikigai_markdown::lint::{expectations, ExpectKind};
/// let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
/// <urn:ex:m> a md:Mapping ; md:construct """
/// PREFIX md: <https://ikigai-rs.dev/ns/md#>
/// CONSTRUCT { ?d <urn:ex:owner> ?o }
/// WHERE { ?d md:field ?f . ?f md:key "owner" ; md:value ?o .
///         ?h a md:Heading ; md:document ?d ; md:text "Summary" }""" ."##;
/// let e = expectations(ttl, "urn:ex:m").unwrap();
/// let found: Vec<(ExpectKind, &str)> = e.expects.keys().map(|x| (x.kind.clone(), x.value.as_str())).collect();
/// assert_eq!(found, vec![(ExpectKind::FrontmatterKey, "owner"), (ExpectKind::Heading, "Summary")]);
/// ```
pub fn expectations(turtle: &str, base: &str) -> Result<Expectations, String> {
    Ok(expectations_of(&read(turtle, base)?, base))
}

pub(crate) fn expectations_of(read: &Read, base: &str) -> Expectations {
    let mut out = Expectations {
        mapping: base.to_string(),
        prefixes: read.raw.prefixes.clone(),
        ..Expectations::default()
    };
    let profile = &read.raw.profile;
    out.splits = profile
        .splits
        .iter()
        .map(|s| (s.key.clone(), s.delimiter.as_str().to_string()))
        .collect();
    out.citations = profile
        .citations
        .iter()
        .map(|c| (c.name.clone(), c.pattern.as_str().to_string()))
        .collect();
    for c in &read.constructs {
        if let Some(problem) = &c.problem {
            out.unreadable.push(problem.clone());
            continue;
        }
        for e in &c.expects {
            out.expects.entry(e.clone()).or_default().insert(c.index);
        }
        if c.reads_any_citation {
            out.any_citation.insert(c.index);
        }
        out.writes.extend(c.produces.predicates.iter().cloned());
        out.types.extend(c.produces.classes.iter().cloned());
    }
    if let Some(shapes) = &read.shapes {
        out.shapes = shapes.shapes.iter().map(|s| s.name.clone()).collect();
    }
    out
}

impl Expectations {
    /// Whether a frontmatter key is one the mapping reads: a split key, or a key some
    /// construct compares (case-insensitively when it folds case, or when it computes
    /// from the key first, since the linter cannot run that computation).
    pub fn knows_key(&self, key: &str) -> bool {
        if self.splits.iter().any(|(k, _)| k == key) {
            return true;
        }
        self.expects.keys().any(|e| match e.kind {
            ExpectKind::FrontmatterKey => matches_how(e, key),
            ExpectKind::Value if e.on.starts_with("computed from md:key") => {
                e.value.eq_ignore_ascii_case(key) || matches_how(e, key)
            }
            _ => false,
        })
    }

    /// The headings the mapping reads by exact text: (text, ignoring case).
    pub fn headings(&self) -> Vec<(&str, bool)> {
        self.expects
            .keys()
            .filter(|e| e.kind == ExpectKind::Heading)
            .filter_map(|e| match e.how {
                How::Equals => Some((e.value.as_str(), false)),
                How::EqualsIgnoringCase => Some((e.value.as_str(), true)),
                _ => None,
            })
            .collect()
    }

    /// The heading levels the mapping reads.
    pub fn heading_levels(&self) -> BTreeSet<i64> {
        self.expects
            .keys()
            .filter(|e| e.kind == ExpectKind::HeadingLevel && e.how == How::Equals)
            .filter_map(|e| e.value.parse().ok())
            .collect()
    }

    /// The constructs that read citations named `name`: those that compare `md:matchedBy`
    /// with it, and those that read every name.
    fn readers_of(&self, name: &str) -> BTreeSet<usize> {
        let mut out = self.any_citation.clone();
        for (e, cs) in &self.expects {
            if e.kind == ExpectKind::CitationName && e.value == name {
                out.extend(cs.iter().copied());
            }
        }
        out
    }

    fn of(&self, kind: ExpectKind) -> Vec<(&Expect, &BTreeSet<usize>)> {
        self.expects
            .iter()
            .filter(|(e, _)| e.kind == kind)
            .collect()
    }

    /// The text face: one section per kind of thing a document must keep.
    ///
    /// ```
    /// let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
    /// <urn:ex:m> a md:Mapping ; md:split <urn:ex:m:s> ; md:construct """
    /// PREFIX md: <https://ikigai-rs.dev/ns/md#>
    /// CONSTRUCT { ?d <urn:ex:tag> ?t } WHERE { ?d md:field ?f . ?f md:key "tags" ; md:token [ md:text ?t ] }""" .
    /// <urn:ex:m:s> md:key "tags" ; md:delimiter ";" ."##;
    /// let text = ikigai_markdown::lint::expectations(ttl, "urn:ex:m").unwrap().to_text();
    /// assert!(text.contains("\nFrontmatter keys\n  tags  equals  (construct 1)\n"), "{text}");
    /// assert!(text.contains("\nSplit frontmatter keys (the lift profile)\n  tags  split on `;`\n"), "{text}");
    /// ```
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "markdown lint: what the mapping expects\n  mapping: {}\n",
            self.mapping
        );
        let section = |out: &mut String, title: &str, rows: Vec<String>| {
            if rows.is_empty() {
                return;
            }
            out.push_str(&format!("\n{title}\n"));
            for r in rows {
                out.push_str(&format!("  {r}\n"));
            }
        };
        let rows = |items: Vec<(&Expect, &BTreeSet<usize>)>, with_on: bool| -> Vec<String> {
            items
                .into_iter()
                .map(|(e, cs)| {
                    let on = if with_on {
                        format!("{}  ", e.on)
                    } else {
                        String::new()
                    };
                    format!("{on}{}  {}  ({})", e.value, e.how.as_str(), list(cs))
                })
                .collect()
        };
        section(
            &mut out,
            "Document paths (md:path)",
            rows(self.of(ExpectKind::Path), false),
        );
        section(
            &mut out,
            "Frontmatter keys",
            rows(self.of(ExpectKind::FrontmatterKey), false),
        );
        section(
            &mut out,
            "Split frontmatter keys (the lift profile)",
            self.splits
                .iter()
                .map(|(k, d)| format!("{k}  split on `{d}`"))
                .collect(),
        );
        section(
            &mut out,
            "Headings, by text",
            rows(self.of(ExpectKind::Heading), false),
        );
        section(
            &mut out,
            "Heading levels",
            rows(self.of(ExpectKind::HeadingLevel), false),
        );
        section(
            &mut out,
            "Citation forms (the lift profile)",
            self.citations
                .iter()
                .map(|(name, pattern)| {
                    let cs = self.readers_of(name);
                    if cs.is_empty() {
                        format!("{name}  `{pattern}`  (no construct reads it)")
                    } else {
                        format!("{name}  `{pattern}`  (read by {})", list(&cs))
                    }
                })
                .collect(),
        );
        section(
            &mut out,
            "Other values the constructs compare with a document's text",
            rows(self.of(ExpectKind::Value), true),
        );
        section(
            &mut out,
            "Classes written",
            self.types
                .iter()
                .map(|t| abbreviate(&short(t), &self.prefixes))
                .collect(),
        );
        section(
            &mut out,
            "Predicates written",
            self.writes
                .iter()
                .map(|p| abbreviate(&short(p), &self.prefixes))
                .collect(),
        );
        section(
            &mut out,
            "Shape",
            if self.shapes.is_empty() {
                vec!["no shape stated (md:shape)".to_string()]
            } else {
                self.shapes
                    .iter()
                    .map(|s| abbreviate(s, &self.prefixes))
                    .collect()
            },
        );
        section(
            &mut out,
            "Constructs that could not be read",
            self.unreadable.clone(),
        );
        out
    }

    /// The Turtle face: one `md:Expectation` per literal, named by a digest of what it
    /// says, plus the profile's splits and citations as they appear in the mapping.
    pub fn to_turtle(&self) -> Result<String, String> {
        let root = node(&format!(
            "urn:markdown:lint:expects:{}",
            digest(&self.to_text())
        ));
        let mut triples = vec![
            Triple::new(root.clone(), rdf_type(), md("LintReport")),
            Triple::new(root.clone(), md("check"), Literal::from("expects")),
            Triple::new(
                root.clone(),
                md("mappingRef"),
                Literal::from(self.mapping.as_str()),
            ),
        ];
        for (e, cs) in &self.expects {
            let key = format!(
                "{}\n{}\n{}\n{}",
                e.kind.as_str(),
                e.how.as_str(),
                e.on,
                e.value
            );
            let n = node(&format!("urn:markdown:lint:expectation:{}", digest(&key)));
            triples.push(Triple::new(root.clone(), md("expects"), n.clone()));
            triples.push(Triple::new(n.clone(), rdf_type(), md("Expectation")));
            triples.push(Triple::new(
                n.clone(),
                md("kind"),
                Literal::from(e.kind.as_str()),
            ));
            triples.push(Triple::new(
                n.clone(),
                md("how"),
                Literal::from(e.how.as_str()),
            ));
            triples.push(Triple::new(
                n.clone(),
                md("value"),
                Literal::from(e.value.as_str()),
            ));
            if !e.on.is_empty() {
                triples.push(Triple::new(
                    n.clone(),
                    md("on"),
                    Literal::from(e.on.as_str()),
                ));
            }
            for c in cs {
                triples.push(Triple::new(n.clone(), md("inConstruct"), integer(*c)));
            }
        }
        for (key, delimiter) in &self.splits {
            let n = node(&format!(
                "urn:markdown:lint:expectation:{}",
                digest(&format!("split\n{key}"))
            ));
            triples.push(Triple::new(root.clone(), md("expects"), n.clone()));
            triples.push(Triple::new(n.clone(), rdf_type(), md("Split")));
            triples.push(Triple::new(
                n.clone(),
                md("key"),
                Literal::from(key.as_str()),
            ));
            triples.push(Triple::new(
                n,
                md("delimiter"),
                Literal::from(delimiter.as_str()),
            ));
        }
        for (name, pattern) in &self.citations {
            let n = node(&format!(
                "urn:markdown:lint:expectation:{}",
                digest(&format!("citation\n{name}"))
            ));
            triples.push(Triple::new(root.clone(), md("expects"), n.clone()));
            triples.push(Triple::new(n.clone(), rdf_type(), md("Citation")));
            triples.push(Triple::new(
                n.clone(),
                md("name"),
                Literal::from(name.as_str()),
            ));
            triples.push(Triple::new(
                n,
                md("pattern"),
                Literal::from(pattern.as_str()),
            ));
        }
        turtle(triples)
    }
}

/// Whether `value` meets one expectation's comparison.
fn matches_how(e: &Expect, value: &str) -> bool {
    match e.how {
        How::Equals => value == e.value,
        How::EqualsIgnoringCase => value.to_lowercase() == e.value.to_lowercase(),
        // A SPARQL regex is XPath-flavored; the common subset reads the same in Rust's, and
        // one that does not compile here matches nothing rather than everything.
        How::Matches => regex::Regex::new(&e.value).is_ok_and(|r| r.is_match(value)),
        How::Contains => value.contains(&e.value),
        How::StartsWith => value.starts_with(&e.value),
        How::EndsWith => value.ends_with(&e.value),
    }
}
