//! The mapping linter: `urn:markdown:lint:mapping`, `:expects`, `:snapshot` and `:corpus`,
//! over the example mapping and the invented decision records in `tests/fixtures/example`.
//!
//! The corpus tests take a snapshot of the fixtures as they are, change the documents the
//! way a spec author changes a format, and pin what the check says, byte for byte: the
//! report is sorted and deterministic, so a golden text is the honest assertion.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use ikigai_core::{ArgRef, Capability, EndpointSpace, Error, Exact, Iri, Kernel, Request, Verb};
use ikigai_markdown::lint::{check_mapping, expectations, Severity};
use ikigai_markdown::{EvaluationBudget, MappingHome, TimeBudget};
use oxigraph::io::{RdfFormat, RdfParser};

const ROOT: &str = "urn:test:corpus:";
const BASELINE: &str = "urn:test:baseline";

fn example() -> String {
    std::fs::read_to_string(mapping_path()).unwrap()
}

/// A kernel over the module's space with `docs` bound under [`ROOT`], the mapping at
/// [`MAPPING_IRI`], and a baseline at [`BASELINE`] when there is one.
fn kernel_with(
    space: EndpointSpace,
    docs: &[(String, String)],
    mapping: &str,
    baseline: Option<&[u8]>,
) -> Kernel {
    let mut space = space.bind(
        Exact::new(MAPPING_IRI),
        file("mapping", "text/turtle", mapping.as_bytes().to_vec()),
    );
    for (i, (path, text)) in docs.iter().enumerate() {
        space = space.bind(
            Exact::new(format!("{ROOT}{path}").as_str()),
            file(
                &format!("doc{i}"),
                "text/markdown",
                text.as_bytes().to_vec(),
            ),
        );
    }
    if let Some(b) = baseline {
        space = space.bind(
            Exact::new(BASELINE),
            file("baseline", "application/n-quads", b.to_vec()),
        );
    }
    Kernel::new(Arc::new(space))
}

fn kernel(docs: &[(String, String)], mapping: &str, baseline: Option<&[u8]>) -> Kernel {
    kernel_with(ikigai_markdown::space(), docs, mapping, baseline)
}

fn list(docs: &[(String, String)]) -> String {
    docs.iter()
        .map(|(p, _)| format!("{ROOT}{p}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn call(kernel: &Kernel, target: &str, args: &[(&str, &str)]) -> Result<String, Error> {
    call_as(kernel, target, args, &Capability::root())
}

fn call_as(
    kernel: &Kernel,
    target: &str,
    args: &[(&str, &str)],
    capability: &Capability,
) -> Result<String, Error> {
    let mut request = Request::new(Verb::Source, Iri::parse(target).unwrap());
    for (name, value) in args {
        request = request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec()));
    }
    futures::executor::block_on(kernel.issue(request, capability))
        .map(|r| String::from_utf8(r.bytes).unwrap())
}

/// A snapshot of `docs` under the example mapping.
fn snapshot(docs: &[(String, String)]) -> String {
    let k = kernel(docs, &example(), None);
    call(
        &k,
        ikigai_markdown::SNAPSHOT_IRI,
        &[
            ("docs", &list(docs)),
            ("root", ROOT),
            ("mapping", MAPPING_IRI),
        ],
    )
    .unwrap()
}

/// The corpus check of `now` against a snapshot of `then`, under `mapping`.
fn check(
    then: &[(String, String)],
    now: &[(String, String)],
    mapping: &str,
    extra: &[(&str, &str)],
) -> String {
    let baseline = snapshot(then);
    let k = kernel(now, mapping, Some(baseline.as_bytes()));
    let docs = list(now);
    let mut args = vec![
        ("docs", docs.as_str()),
        ("root", ROOT),
        ("mapping", MAPPING_IRI),
        ("baseline", BASELINE),
    ];
    args.extend_from_slice(extra);
    call(&k, ikigai_markdown::LINT_CORPUS_IRI, &args).unwrap()
}

// --- check 1: the mapping on its own ----------------------------------------------------

#[test]
fn the_example_mapping_checks_clean() {
    let report = check_mapping(&example(), "urn:test:mapping");
    assert_eq!(
        report.to_text(),
        "markdown lint: mapping\n  mapping: urn:test:mapping\n  profile: 1 split(s), 1 \
         citation pattern(s)\n  constructs: 4\n  result: 0 errors, 0 warnings, 0 notes\n"
    );
}

#[test]
fn every_problem_in_a_mapping_is_reported_at_once() {
    // The lift refuses on the FIRST of these; the linter lists them all.
    let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
@prefix ex: <http://example.org/ns#> .
<urn:ex:m> a md:Mapping ;
  md:split <urn:ex:m:s> ;
  md:citation <urn:ex:m:c1> , <urn:ex:m:c2> ;
  md:construct "SELECT * WHERE { ?s ?p ?o }" ;
  md:construct """PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { [] <http://example.org/ns#seen> ?d } WHERE { ?d a md:Document }""" ;
  md:construct """PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { ?d <http://example.org/ns#cites> ?v }
WHERE { ?m md:document ?d ; md:matchedBy "ticket" ; md:value ?v }""" ;
  md:construct "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p" .
<urn:ex:m:s> md:key "tags" ; md:delimiter "(" .
<urn:ex:m:c1> md:name "issue" ; md:pattern "#[0-9]+" .
<urn:ex:m:c2> md:name "broken" ; md:pattern "[a-" ."##;
    let report = check_mapping(ttl, "urn:ex:m");
    let codes: Vec<(Severity, &str)> = report
        .findings
        .iter()
        .map(|f| (f.severity, f.code.as_str()))
        .collect();
    assert_eq!(
        codes,
        vec![
            (Severity::Error, "construct-blank-node"),
            (Severity::Error, "construct-invalid"),
            (Severity::Error, "construct-invalid"),
            (Severity::Error, "profile-invalid"),
            (Severity::Error, "profile-invalid"),
            (Severity::Warning, "citation-undefined"),
            (Severity::Info, "citation-unread"),
            (Severity::Info, "citation-whole-match"),
            (Severity::Info, "no-shape"),
        ],
        "{}",
        report.to_text()
    );
    let text = report.to_text();
    for expected in [
        "construct 1 is not a CONSTRUCT query",
        "construct 4 is not valid SPARQL",
        "md:split `tags`: delimiter is not a regex: regex parse error: ( ^ error: unclosed group",
        "md:citation `broken`: pattern is not a regex",
        "construct 3 compare(s) md:matchedBy with `ticket`, which the profile defines no \
         md:citation for",
        "md:citation `issue`: no group `v` and no group 1, so md:value is the whole match",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
}

#[test]
fn what_a_construct_writes_is_checked_against_the_stated_shape() {
    // Take dr:title out of the shape: construct 1 still writes it, and a shape that asks
    // for something no construct writes is reported too.
    let ttl = example()
        .replace(
            "                <urn:example:mapping:shape:decision:title> ,\n",
            "                <urn:example:mapping:shape:decision:owner> ,\n",
        )
        .replace(
            "<urn:example:mapping:shape:decision:title> sh:path dr:title ;",
            "<urn:example:mapping:shape:decision:owner> sh:path dr:owner ;",
        );
    assert_ne!(ttl, example(), "the edit applied");
    let text = check_mapping(&ttl, "urn:test:mapping").to_text();
    assert!(
        text.ends_with(
            "  result: 0 errors, 2 warnings, 0 notes\n\n(the mapping)\n\
             \x20 warning  shape-unproduced  <urn:example:mapping:shape:decision> asks for \
             dr:owner (sh:minCount 1), and no construct writes it\n\
             \x20 warning  undeclared-predicate  construct 1 writes dr:title, which no shape \
             declares\n"
        ),
        "{text}"
    );
}

#[test]
fn a_mapping_with_no_shape_says_so() {
    let ttl = example().replace("    md:shape <urn:example:mapping:shape:decision> ;\n", "");
    let text = check_mapping(&ttl, "urn:test:mapping").to_text();
    assert!(
        text.contains(
            "  info     no-shape  no shape stated: the mapping has no md:shape, so what its \
             constructs write is not checked\n"
        ),
        "{text}"
    );
}

#[test]
fn turtle_that_does_not_parse_is_a_finding_through_the_endpoint() {
    let k = kernel(&[], "not turtle at all", None);
    let text = call(
        &k,
        ikigai_markdown::LINT_MAPPING_IRI,
        &[("mapping", MAPPING_IRI)],
    )
    .expect("a linter answers about a broken mapping; it does not refuse it");
    assert!(
        text.contains("error    mapping-unreadable  mapping is not valid Turtle"),
        "{text}"
    );
}

// --- check 3: what the mapping expects --------------------------------------------------

#[test]
fn what_the_example_mapping_expects() {
    let text = expectations(&example(), "urn:test:mapping")
        .unwrap()
        .to_text();
    assert_eq!(text, EXPECTS, "\n{text}");
}

const EXPECTS: &str = "markdown lint: what the mapping expects
  mapping: urn:test:mapping

Document paths (md:path)
  (^|/)decisions/[0-9]{4}-[^/]*[.]md$  matches  (construct 1, 2, 3, 4)

Frontmatter keys
  date  equals-ignoring-case  (construct 2)
  related  equals  (construct 3)
  status  equals-ignoring-case  (construct 2)

Split frontmatter keys (the lift profile)
  related  split on `,`

Headings, by text
  Record  equals  (construct 2)

Heading levels
  1  equals  (construct 1)

Citation forms (the lift profile)
  decision  `DR-(?P<v>[0-9]{4})`  (read by construct 4)

Other values the constructs compare with a document's text
  computed from md:raw of md:ListItem  ^[0-9]{4}-[0-9]{2}-[0-9]{2}$  matches  (construct 2)
  computed from md:raw of md:ListItem  date  equals-ignoring-case  (construct 2)
  computed from md:raw of md:ListItem  status  equals-ignoring-case  (construct 2)
  computed from md:value of md:Field  ^[0-9]{4}-[0-9]{2}-[0-9]{2}$  matches  (construct 2)
  md:raw of md:ListItem  ^[*][*][^*]+[*][*]:  matches  (construct 2)
  md:text of md:Token  ^[0-9]{4}$  matches  (construct 3)

Classes written
  dr:Decision

Predicates written
  dr:cites
  dr:date
  dr:document
  dr:number
  dr:related
  dr:status
  dr:title
  rdf:type

Shape
  <urn:example:mapping:shape:decision>
";

#[test]
fn the_turtle_faces_use_only_vocabulary_terms_and_no_blank_nodes() {
    let k = kernel(&corpus(&[]), &example(), None);
    let docs = list(&corpus(&[]));
    let faces = [
        call(
            &k,
            ikigai_markdown::LINT_EXPECTS_IRI,
            &[("mapping", MAPPING_IRI), ("as", "text/turtle")],
        )
        .unwrap(),
        call(
            &k,
            ikigai_markdown::LINT_MAPPING_IRI,
            &[
                (
                    "mapping",
                    "@prefix md: <https://ikigai-rs.dev/ns/md#> . <urn:x> a md:Mapping .",
                ),
                ("as", "text/turtle"),
            ],
        )
        .unwrap(),
        call(
            &k,
            ikigai_markdown::LINT_CORPUS_IRI,
            &[
                ("docs", &docs),
                ("root", ROOT),
                ("mapping", MAPPING_IRI),
                ("as", "text/turtle"),
            ],
        )
        .unwrap(),
    ];
    for face in faces {
        let mut terms = std::collections::BTreeSet::new();
        for t in RdfParser::from_format(RdfFormat::Turtle).for_slice(face.as_bytes()) {
            let t = t.unwrap_or_else(|e| panic!("{e}\n{face}"));
            assert!(
                !t.subject.is_blank_node() && !t.object.is_blank_node(),
                "a blank node in\n{face}"
            );
            terms.insert(t.predicate.as_str().to_string());
            if let oxigraph::model::Term::NamedNode(n) = &t.object {
                terms.insert(n.as_str().to_string());
            }
        }
        let missing: Vec<&str> = terms
            .iter()
            .filter_map(|t| t.strip_prefix(ikigai_markdown::MD))
            .filter(|l| !ikigai_markdown::VOCABULARY.contains(&format!("md:{l} a ")))
            .collect();
        assert!(
            missing.is_empty(),
            "undefined md: terms {missing:?} in\n{face}"
        );
    }
}

// --- the snapshot -------------------------------------------------------------------------

#[test]
fn a_snapshot_is_one_graph_per_document_named_by_its_path() {
    let snap = snapshot(&corpus(&[]));
    let mut graphs = std::collections::BTreeSet::new();
    for q in RdfParser::from_format(RdfFormat::NQuads).for_slice(snap.as_bytes()) {
        graphs.insert(q.unwrap().graph_name.to_string());
    }
    assert_eq!(
        graphs.into_iter().collect::<Vec<_>>(),
        vec![
            "<urn:markdown:doc:decisions/0001-use-turtle-for-mappings.md>",
            "<urn:markdown:doc:decisions/0002-cache-every-lift.md>",
            "<urn:markdown:doc:decisions/0003-name-mappings-in-config.md>",
        ]
    );
    let mut sorted: Vec<&str> = snap.lines().collect();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted,
        snap.lines().collect::<Vec<_>>(),
        "sorted, each line once"
    );
}

#[test]
fn a_full_lift_compares_equal_to_its_snapshot() {
    // The snapshot keeps only what the check reads. Comparing it with the FULL lift of
    // the same documents finds nothing, so the reduction loses nothing the check uses.
    let docs = corpus(&[]);
    let k = kernel(&docs, &example(), None);
    let mut full = String::new();
    for (path, _) in &docs {
        full.push_str(
            &call(
                &k,
                ikigai_markdown::LIFT_IRI,
                &[
                    ("src", &format!("{ROOT}{path}")),
                    ("path", path),
                    ("mapping", MAPPING_IRI),
                ],
            )
            .unwrap(),
        );
    }
    assert!(
        full.len() > 2 * snapshot(&docs).len(),
        "the snapshot is smaller"
    );
    let k = kernel(&docs, &example(), Some(full.as_bytes()));
    let text = call(
        &k,
        ikigai_markdown::LINT_CORPUS_IRI,
        &[
            ("docs", &list(&docs)),
            ("root", ROOT),
            ("mapping", MAPPING_IRI),
            ("baseline", BASELINE),
        ],
    )
    .unwrap();
    assert!(
        text.ends_with("  result: 0 errors, 0 warnings, 0 notes\n"),
        "{text}"
    );
}

// --- check 2: the corpus against a baseline ----------------------------------------------

#[test]
fn an_unchanged_corpus_reports_nothing() {
    let docs = corpus(&[]);
    assert_eq!(
        check(&docs, &docs, &example(), &[]),
        "markdown lint: corpus\n  mapping: urn:test:mapping\n  baseline: urn:test:baseline\n  \
         documents: 3 now, 3 at the baseline\n  result: 0 errors, 0 warnings, 0 notes\n"
    );
}

const RECORD_AS_DETAILS: &str = "# Cache every lift

## Details

- **Status**: Accepted
- **Date**: 2026-02-03

## Context

A lift is a pure function of the document and the mapping, as decision 1 made the
mapping a resource. Recomputing it on every read wastes the work.

## Decision

Cache the lift under both inputs' golden threads. The measurements live in DR-0007.
";

#[test]
fn a_changed_format_is_reported_document_by_document() {
    // A spec author renames a heading, rewrites a citation, adds a frontmatter key and
    // changes a value. Each lands under its document, in a stable order.
    let then = corpus(&[]);
    let now = corpus(&[
        ("decisions/0002-cache-every-lift.md", RECORD_AS_DETAILS),
        (
            "decisions/0001-use-turtle-for-mappings.md",
            "---\nstatus: rejected\ndate: 2026-01-12\nrelated: 0002\nowner: someone\n---\n\n\
             # Use Turtle for mappings\n\nA mapping is data.\n",
        ),
    ]);
    let text = check(&then, &now, &example(), &[]);
    assert_eq!(text, CHANGED, "\n{text}");
    assert_eq!(check(&then, &now, &example(), &[]), text, "deterministic");
}

const CHANGED: &str = "markdown lint: corpus
  mapping: urn:test:mapping
  baseline: urn:test:baseline
  documents: 3 now, 3 at the baseline
  result: 6 errors, 2 warnings, 0 notes

decisions/0001-use-turtle-for-mappings.md
  warning  field-changed  <urn:example:decision:0001> dr:status: \"accepted\" → \"rejected\"
  warning  key-unknown  frontmatter key `owner` is new since the baseline, and the mapping does not read it

decisions/0002-cache-every-lift.md
  error    citation-lost  citation `decision` \"0001\" is no longer recognized; at the baseline it read \"DR-0001\" on line 10
  error    field-lost  <urn:example:decision:0002> dr:cites <urn:example:decision:0001> is no longer produced
  error    field-lost  <urn:example:decision:0002> dr:date \"2026-02-03\"^^xsd:date is no longer produced
  error    field-lost  <urn:example:decision:0002> dr:status \"accepted\" is no longer produced
  error    heading-vanished  heading \"Record\" is gone, and the mapping reads it (it was a level-2 heading under \"Cache every lift\")
  error    shape-violation  <urn:example:decision:0002> dr:status: 0 value(s), and <urn:example:mapping:shape:decision> asks for at least 1 (sh:minCount)
";

#[test]
fn a_heading_that_moved_is_a_warning() {
    let moved = "# Cache every lift\n\n## Background\n\n### Record\n\n- **Status**: Accepted\n\
                 - **Date**: 2026-02-03\n\nAs DR-0001 made the mapping a resource. See DR-0007.\n";
    let now = corpus(&[("decisions/0002-cache-every-lift.md", moved)]);
    let text = check(&corpus(&[]), &now, &example(), &[]);
    assert!(
        text.contains(
            "  warning  heading-moved  heading \"Record\", which the mapping reads, moved: level \
             2 → 3; under \"Cache every lift\" → under \"Background\"\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("  result: 0 errors, 1 warning, 0 notes\n"),
        "{text}"
    );
}

#[test]
fn frontmatter_that_no_longer_parses_is_an_error() {
    let now = corpus(&[(
        "decisions/0001-use-turtle-for-mappings.md",
        "---\nstatus: [accepted\n---\n\n# Use Turtle for mappings\n",
    )]);
    let text = check(&corpus(&[]), &now, &example(), &[]);
    assert!(
        text.contains(
            "  error    frontmatter-unreadable  the frontmatter no longer parses as YAML:"
        ),
        "{text}"
    );
}

#[test]
fn an_ignored_predicate_is_left_out_by_its_prefixed_name() {
    let now = corpus(&[(
        "decisions/0001-use-turtle-for-mappings.md",
        "---\nstatus: rejected\ndate: 2026-01-12\nrelated: 0002\n---\n\n# Use Turtle for mappings\n\n\
         A mapping is data, so it should be a graph like everything else. Turtle is the\n\
         format every other part of the system already reads.\n\n## Consequences\n\n\
         Mappings can be queried, diffed and validated with the tools that already exist.\n",
    )]);
    let then = corpus(&[]);
    assert!(check(&then, &now, &example(), &[]).contains("field-changed"));
    let text = check(&then, &now, &example(), &[("ignore", "dr:status")]);
    assert!(
        text.contains("  ignored predicates: <http://example.org/decisions#status>\n")
            && text.ends_with("  result: 0 errors, 0 warnings, 0 notes\n"),
        "{text}"
    );
}

#[test]
fn a_document_the_lift_cannot_read_is_reported_not_refused() {
    let docs = corpus(&[]);
    let mut space = ikigai_markdown::space();
    space = space.bind(
        Exact::new(format!("{ROOT}decisions/0004-binary.md").as_str()),
        file("binary", "text/markdown", vec![b'#', b' ', 0xff, 0xfe]),
    );
    let k = kernel_with(space, &docs, &example(), None);
    let listed = format!("{}\n{ROOT}decisions/0004-binary.md", list(&docs));
    let text = call(
        &k,
        ikigai_markdown::LINT_CORPUS_IRI,
        &[("docs", &listed), ("root", ROOT), ("mapping", MAPPING_IRI)],
    )
    .unwrap();
    assert!(
        text.contains(
            "decisions/0004-binary.md\n  error    document-unreadable  the lift cannot read this \
             document: src is not valid UTF-8"
        ),
        "{text}"
    );
}

#[test]
fn a_mapping_that_does_not_load_is_a_finding_not_a_refusal() {
    let docs = corpus(&[]);
    let broken = example().replace("DR-(?P<v>[0-9]{4})", "DR-(?P<v>[0-9]{4}");
    let k = kernel(&docs, &broken, None);
    let text = call(
        &k,
        ikigai_markdown::LINT_CORPUS_IRI,
        &[
            ("docs", &list(&docs)),
            ("root", ROOT),
            ("mapping", MAPPING_IRI),
        ],
    )
    .unwrap();
    assert!(
        text.contains("  error    mapping-unusable  the corpus was not lifted"),
        "{text}"
    );
    assert!(
        text.contains("  error    profile-invalid  md:citation `decision`"),
        "{text}"
    );
}

// --- bounds: refuse, never answer for part of a corpus ------------------------------------

fn refusal(kernel: &Kernel, args: &[(&str, &str)]) -> (String, String) {
    match call(kernel, ikigai_markdown::SNAPSHOT_IRI, args) {
        Err(Error::InvalidArgument { name, detail }) => (name, detail),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_corpus_past_the_document_count_is_refused() {
    let k = kernel(&[], &example(), None);
    let many: Vec<String> = (0..=ikigai_markdown::MAX_DOCUMENTS)
        .map(|i| format!("{ROOT}d{i}.md"))
        .collect();
    let (name, detail) = refusal(&k, &[("docs", &many.join("\n"))]);
    assert_eq!(name, "docs");
    assert!(detail.contains("(MAX_DOCUMENTS)"), "{detail}");
}

#[test]
fn a_list_past_its_byte_bound_is_refused() {
    let k = kernel(&[], &example(), None);
    let long = "x".repeat(ikigai_markdown::MAX_LIST_BYTES + 1);
    let (name, detail) = refusal(&k, &[("docs", &long)]);
    assert_eq!(name, "docs");
    assert!(detail.contains("(MAX_LIST_BYTES)"), "{detail}");
}

#[test]
fn a_document_past_its_byte_bound_is_refused() {
    let big = vec![(
        "big.md".to_string(),
        "a".repeat(ikigai_markdown::MAX_DOCUMENT_BYTES + 1),
    )];
    let k = kernel(&big, &example(), None);
    let (name, detail) = refusal(&k, &[("docs", &list(&big)), ("root", ROOT)]);
    assert_eq!(name, "docs");
    assert!(detail.contains("(MAX_DOCUMENT_BYTES)"), "{detail}");
}

#[test]
fn a_document_outside_the_root_is_refused() {
    let docs = corpus(&[]);
    let k = kernel(&docs, &example(), None);
    let (name, detail) = refusal(
        &k,
        &[("docs", &list(&docs)), ("root", "urn:test:elsewhere:")],
    );
    assert_eq!(name, "docs");
    assert!(
        detail.contains("is not under the root `urn:test:elsewhere:`"),
        "{detail}"
    );
}

#[test]
fn a_corpus_past_the_callers_time_budget_is_refused() {
    let docs = corpus(&[]);
    let budget = EvaluationBudget::new(TimeBudget::new(Duration::ZERO));
    let space = ikigai_markdown::space_with_budget(Arc::new(MappingHome::new(None)), budget);
    let k = kernel_with(space, &docs, &example(), None);
    let caller = Capability::scoped(std::iter::empty::<String>());
    let err = call_as(
        &k,
        ikigai_markdown::SNAPSHOT_IRI,
        &[
            ("docs", &list(&docs)),
            ("root", ROOT),
            ("mapping", MAPPING_IRI),
        ],
        &caller,
    )
    .unwrap_err();
    let Error::Timeout(detail) = err else {
        panic!("{err}");
    };
    assert!(
        detail.starts_with("the snapshot (at document 1 of 3) ran past the caller's time budget"),
        "{detail}"
    );
}
