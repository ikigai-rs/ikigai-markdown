//! Stage 1, the generic structural lift, on its own: what the graph says about a
//! document when no mapping interprets it, and the properties that make it safe to
//! cache and diff.

mod common;

use common::*;
use ikigai_markdown::{lift, DocRef, Mapping, Profile};
use oxigraph::model::{GraphName, NamedNode, Quad};
use oxigraph::store::Store;

const MD_PREFIX: &str = "PREFIX md: <https://ikigai-rs.dev/ns/md#>\n";

fn doc(path: &str) -> DocRef {
    DocRef {
        iri: NamedNode::new(format!("urn:markdown:doc:{path}")).unwrap(),
        path: path.to_string(),
        source: None,
    }
}

fn store_of(quads: &[Quad]) -> Store {
    let store = Store::new().unwrap();
    for q in quads {
        store.insert(q).unwrap();
    }
    store
}

fn ask(quads: &[Quad], select_body: &str) -> Vec<Vec<String>> {
    select(&store_of(quads), &format!("{MD_PREFIX}{select_body}"))
}

const SAMPLE: &str = "---
title: Sample
number: 0002
tags: [one, two]
owner:
  name: Someone
list: a; b ;; c
---

# Top

Intro paragraph with a [link](https://example.org/x) and `code`.

## Details

- **Key**: first value
  continues here
  <!--
  - **Key**: hidden
  -->
- plain item
  - nested item
- [x] done task

1. ordered

```sparql
SELECT * WHERE { ?s ?p ?o }
```

| a | b |
|---|---|
| 1 | 2 |

> quoted

---

### Deeper

Tail.
";

#[test]
fn frontmatter_keeps_the_text_the_author_wrote() {
    let profile = Mapping::parse(
        r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
           <#m> a md:Mapping ; md:split [ md:key "list" ; md:delimiter ";" ] ."#,
        "urn:test:m",
    )
    .unwrap()
    .profile;
    let quads = lift(SAMPLE, &doc("s.md"), &profile);
    assert_eq!(
        ask(
            &quads,
            "SELECT ?key ?value ?line WHERE { ?f a md:Field ; md:key ?key ; md:line ?line
             OPTIONAL { ?f md:value ?value } } ORDER BY ?line ?key"
        ),
        vec![
            row(&["title", "Sample", "2"]),
            // YAML would make this the integer 2; the lift keeps `0002`.
            row(&["number", "0002", "3"]),
            row(&["tags", "", "4"]),
            // Nested keys are dot-joined.
            row(&["owner.name", "Someone", "6"]),
            row(&["list", "a; b ;; c", "7"]),
        ]
    );
    assert_eq!(
        ask(
            &quads,
            "SELECT ?key ?text ?index WHERE { ?f md:key ?key ; md:token ?t . ?t md:text ?text ; md:index ?index }
             ORDER BY ?key ?index"
        ),
        vec![
            // A profile split: trimmed, empties dropped.
            row(&["list", "a", "1"]),
            row(&["list", "b", "2"]),
            row(&["list", "c", "3"]),
            // A YAML sequence becomes tokens with no profile at all.
            row(&["tags", "one", "1"]),
            row(&["tags", "two", "2"]),
        ]
    );
}

#[test]
fn malformed_frontmatter_is_recorded_not_fatal() {
    let quads = lift(
        "---\nkey: [unclosed\n---\n\n# Still here\n",
        &doc("bad.md"),
        &Profile::default(),
    );
    let rows = ask(
        &quads,
        "SELECT ?err ?h WHERE { ?d md:frontmatterError ?err . ?x a md:Heading ; md:text ?h }",
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0][1], "Still here");
}

#[test]
fn sections_nest_and_blocks_know_their_section() {
    let quads = lift(SAMPLE, &doc("s.md"), &Profile::default());
    assert_eq!(
        ask(
            &quads,
            "SELECT ?h ?level ?parent WHERE { ?x a md:Heading ; md:text ?h ; md:level ?level
             OPTIONAL { ?x md:section [ md:text ?parent ] } } ORDER BY ?level"
        ),
        vec![
            row(&["Top", "1", ""]),
            row(&["Details", "2", "Top"]),
            row(&["Deeper", "3", "Details"]),
        ]
    );
    assert_eq!(
        ask(
            &quads,
            "SELECT ?class ?section WHERE { ?b a ?class ; md:section [ md:text ?section ] .
             FILTER(?class IN (md:CodeBlock, md:Table, md:BlockQuote, md:ThematicBreak)) } ORDER BY ?class"
        ),
        vec![
            row(&["https://ikigai-rs.dev/ns/md#BlockQuote", "Details"]),
            row(&["https://ikigai-rs.dev/ns/md#CodeBlock", "Details"]),
            row(&["https://ikigai-rs.dev/ns/md#Table", "Details"]),
            row(&["https://ikigai-rs.dev/ns/md#ThematicBreak", "Details"]),
        ]
    );
}

/// Two headings at the SAME level under one parent, each with their own subtree.
///
/// ★ This is the property every layered document format rests on, and the one whose
/// failure would be silent: if the second `### Item` kept the first one's section, or
/// if `#### Leaf: B` attached to `Item: First`, a mapping would still produce a graph
/// — just one that quietly says the wrong thing. `SAMPLE` cannot catch it, because it
/// has no two headings at the same level anywhere.
///
/// It also pins the distinction between the two containment edges, which are easy to
/// confuse when writing a mapping: `md:parent` is BLOCK containment (a list item's
/// list), so a heading's parent is always the document, however deeply it nests;
/// `md:section` is the HEADING hierarchy.
#[test]
fn heading_siblings_do_not_bleed_into_each_other() {
    const NESTED: &str = "# Doc

## Group

### Item: First

Body of first.

#### Leaf: A

- alpha

### Item: Second

Body of second.

#### Leaf: B

- beta
";
    let quads = lift(NESTED, &doc("n.md"), &Profile::default());

    // Four levels, and the second level-3 heading returns to "Group" rather than
    // nesting under its sibling.
    assert_eq!(
        ask(
            &quads,
            "SELECT ?h ?level ?section WHERE { ?x a md:Heading ; md:text ?h ; md:level ?level ;
               md:order ?o . OPTIONAL { ?x md:section [ md:text ?section ] } } ORDER BY ?o"
        ),
        vec![
            row(&["Doc", "1", ""]),
            row(&["Group", "2", "Doc"]),
            row(&["Item: First", "3", "Group"]),
            row(&["Leaf: A", "4", "Item: First"]),
            row(&["Item: Second", "3", "Group"]),
            row(&["Leaf: B", "4", "Item: Second"]),
        ]
    );

    // Each subtree's content stays in its own subtree: a paragraph under the
    // requirement-shaped heading, a list item under the leaf-shaped one below it.
    assert_eq!(
        ask(
            &quads,
            "SELECT ?text ?section WHERE { ?b a ?c ; md:text ?text ; md:order ?o ;
               md:section [ md:text ?section ] .
               FILTER(?c IN (md:Paragraph, md:ListItem)) } ORDER BY ?o"
        ),
        vec![
            row(&["Body of first.", "Item: First"]),
            row(&["alpha", "Leaf: A"]),
            row(&["Body of second.", "Item: Second"]),
            row(&["beta", "Leaf: B"]),
        ]
    );

    // md:parent is block containment, not heading depth: every heading's parent is
    // the document, and only the list item has a block for a parent.
    assert_eq!(
        ask(
            &quads,
            "SELECT (COUNT(*) AS ?n) WHERE { ?h a md:Heading ; md:parent ?p .
               FILTER(?p = <urn:markdown:doc:n.md>) }"
        ),
        vec![row(&["6"])]
    );
}

#[test]
fn a_list_item_holds_its_own_text_and_nothing_nested() {
    let quads = lift(SAMPLE, &doc("s.md"), &Profile::default());
    assert_eq!(
        ask(
            &quads,
            "SELECT ?text ?raw ?checked WHERE { ?i a md:ListItem ; md:text ?text
             OPTIONAL { ?i md:raw ?raw } OPTIONAL { ?i md:checked ?checked } } ORDER BY ?text"
        ),
        vec![
            row(&[
                "Key: first value continues here",
                "**Key**: first value\n  continues here",
                ""
            ]),
            row(&["done task", "done task", "true"]),
            row(&["nested item", "nested item", ""]),
            row(&["ordered", "ordered", ""]),
            // The nested list is its own block; its text does not leak upward.
            row(&["plain item", "plain item", ""]),
        ]
    );
    // The HTML comment is an HtmlBlock child of the first item, not its text.
    let html = ask(
        &quads,
        "SELECT ?text WHERE { ?h a md:HtmlBlock ; md:text ?text ; md:parent [ a md:ListItem ] }",
    );
    assert_eq!(html.len(), 1);
    assert!(html[0][0].contains("hidden"));
}

#[test]
fn links_code_and_tables_are_structure() {
    let quads = lift(SAMPLE, &doc("s.md"), &Profile::default());
    assert_eq!(
        ask(
            &quads,
            "SELECT ?href ?text ?in WHERE { ?l a md:Link ; md:href ?href ; md:text ?text ; md:block [ md:text ?in ] }"
        ),
        vec![row(&[
            "https://example.org/x",
            "link",
            "Intro paragraph with a link and code."
        ])]
    );
    assert_eq!(
        ask(
            &quads,
            "SELECT ?info ?text WHERE { ?c a md:CodeBlock ; md:info ?info ; md:text ?text }"
        ),
        vec![row(&["sparql", "SELECT * WHERE { ?s ?p ?o }\n"])]
    );
    assert_eq!(
        ask(
            &quads,
            "SELECT ?header ?col ?text WHERE { ?r a md:TableRow ; md:header ?header .
             ?c md:parent ?r ; md:column ?col ; md:text ?text } ORDER BY DESC(?header) ?col"
        ),
        vec![
            row(&["true", "0", "a"]),
            row(&["true", "1", "b"]),
            row(&["false", "0", "1"]),
            row(&["false", "1", "2"]),
        ]
    );
}

#[test]
fn a_citation_pattern_emits_every_match_with_its_line_and_block() {
    let profile = Mapping::parse(
        r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
           <#m> a md:Mapping ;
             md:citation [ md:name "ticket" ; md:pattern "T-(?P<v>[0-9]+)" ] ,
                         [ md:name "whole" ; md:pattern "KEY" ] ."#,
        "urn:test:m",
    )
    .unwrap()
    .profile;
    let text = "# Head T-1\n\nPara T-22 and T-333.\n\n- item KEY\n";
    let quads = lift(text, &doc("c.md"), &profile);
    assert_eq!(
        ask(
            &quads,
            "SELECT ?pattern ?text ?value ?line ?class WHERE { ?m a md:Match ; md:matchedBy ?pattern ;
             md:text ?text ; md:value ?value ; md:line ?line ; md:block [ a ?class ] } ORDER BY ?line ?text"
        ),
        vec![
            row(&["ticket", "T-1", "1", "1", "https://ikigai-rs.dev/ns/md#Heading"]),
            row(&["ticket", "T-22", "22", "3", "https://ikigai-rs.dev/ns/md#Paragraph"]),
            row(&["ticket", "T-333", "333", "3", "https://ikigai-rs.dev/ns/md#Paragraph"]),
            // No group: the value is the whole match. The innermost block wins.
            row(&["whole", "KEY", "KEY", "5", "https://ikigai-rs.dev/ns/md#ListItem"]),
        ]
    );
}

#[test]
fn the_same_bytes_lift_to_the_same_quads_with_no_blank_nodes() {
    let mapping = Mapping::parse(
        &std::fs::read_to_string(mapping_path()).unwrap(),
        "urn:test:mapping",
    )
    .unwrap();
    let text = std::fs::read_to_string(
        repo().join("tests/fixtures/example/decisions/0002-cache-every-lift.md"),
    )
    .unwrap();
    let d = doc("decisions/0002-cache-every-lift.md");
    let run = || {
        let mut quads = lift(&text, &d, &mapping.profile);
        quads.extend(ikigai_markdown::apply(&quads, &d.iri, &mapping.constructs).unwrap());
        ikigai_markdown::serialize(&quads, false).unwrap()
    };
    let first = run();
    assert_eq!(first, run(), "the lift is a function of its inputs");
    let nq = String::from_utf8(first).unwrap();
    assert!(!nq.contains("_:"), "no blank nodes:\n{nq}");
    for line in nq.lines() {
        assert!(
            line.ends_with("<urn:markdown:doc:decisions/0002-cache-every-lift.md> ."),
            "every quad is in the document's graph: {line}"
        );
    }
    let _ = GraphName::DefaultGraph;
}

#[test]
fn a_construct_that_mints_blank_nodes_is_refused() {
    let mapping = Mapping::parse(
        r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
           <#m> a md:Mapping ; md:construct """
             PREFIX md: <https://ikigai-rs.dev/ns/md#>
             CONSTRUCT { [] <urn:ex:p> ?t } WHERE { ?h md:text ?t }""" ."#,
        "urn:test:m",
    )
    .unwrap();
    let d = doc("b.md");
    let quads = lift("# x\n", &d, &mapping.profile);
    let err = ikigai_markdown::apply(&quads, &d.iri, &mapping.constructs).unwrap_err();
    assert!(err.contains("blank-node"), "{err}");
}

#[test]
fn a_mapping_is_refused_with_its_problem_named() {
    let cases = [
        (
            "@prefix md: <https://ikigai-rs.dev/ns/md#> . <#a> md:split [] .",
            "no subject typed md:Mapping",
        ),
        (
            "@prefix md: <https://ikigai-rs.dev/ns/md#> . <#a> a md:Mapping . <#b> a md:Mapping .",
            "exactly one",
        ),
        (
            r#"@prefix md: <https://ikigai-rs.dev/ns/md#> . <#a> a md:Mapping ; md:citation [ md:name "x" ; md:pattern "(" ] ."#,
            "pattern is not a regex",
        ),
        (
            r#"@prefix md: <https://ikigai-rs.dev/ns/md#> . <#a> a md:Mapping ; md:construct "SELECT * WHERE { ?s ?p ?o }" ."#,
            "must be a CONSTRUCT",
        ),
        ("not turtle at all", "not valid Turtle"),
    ];
    for (ttl, expected) in cases {
        let err = Mapping::parse(ttl, "urn:test:m").unwrap_err();
        assert!(err.contains(expected), "{ttl}\n→ {err}");
    }
}

/// ★ The design constraint, as a test: stage 1 knows nothing about any particular
/// document process. Every word a decision-record mapping cares about lives in
/// a mapping, and none of it may appear in the crate's source — not in code, not in
/// comments. If this fails, the branch you just wrote belongs in a mapping.
///
/// The list covers two families, because one family only guards the format that was
/// here first. The second is the vocabulary of SPEC-DRIVEN formats — requirements,
/// scenarios, deltas — which is what a mapping for a layered format would be tempted
/// to teach the lifter. A guard that names only the words already in the repository
/// catches nothing new; these are the words the NEXT dialect would smuggle in.
///
/// ⚠ What this cannot guard, and the reason is worth knowing before trusting it:
/// `capability` is one of ikigai's own kernel concepts and is used in this crate's
/// own documentation, so it can never be denied here. A word this ecosystem already
/// owns is invisible to a word denylist, and the overlap is not a coincidence —
/// document-process vocabularies and this system's vocabulary are drawn from the
/// same well. This test is a tripwire, not a proof.
#[test]
fn no_source_file_knows_a_document_process_vocabulary() {
    let words = regex::Regex::new(
        // ⚠ The boundary is `[^a-zA-Z0-9]`, not `\b`. Regex counts `_` as a word
        // character, so `\b` does not fire inside a snake_case identifier — and
        // snake_case is how format knowledge would actually arrive in Rust. Under
        // `\b`, `fn render_delta()` and `let parse_status` both read as clean. This
        // is the same tokenization `tests/tree_guard.rs` uses, for the same reason.
        r"(?i)(^|[^a-zA-Z0-9])(rdr|rdrs|jdr|jdrs|rfd|rfds|adr|pep|status|state|cluster|supersedes?|predecessors?|overrides?|metadata|seam|lineage|superseded|requirements?|scenarios?|deltas?|proposals?|shall)([^a-zA-Z0-9]|$)",
    )
    .unwrap();

    // Identifiers a DEPENDENCY named, which this crate only spells. They are not
    // this crate knowing a document process; they are the parser's own API surface,
    // and renaming them is not ours to do. Each is removed from the line before the
    // scan, and a second assertion below fails if one goes stale.
    //
    // ⚠ There was exactly one when the boundary above was widened, and it had been
    // in `src/` since the first commit — `\b` never fired on it, because `_` is a
    // word character. A guard that cannot see a snake_case identifier cannot see the
    // shape that format knowledge actually takes in Rust.
    const UPSTREAM: [&str; 1] = ["ENABLE_YAML_STYLE_METADATA_BLOCKS"];

    let mut hits = Vec::new();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(repo().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let mut scanned = line.to_string();
            for id in UPSTREAM {
                scanned = scanned.replace(id, "");
            }
            if let Some(c) = words.captures(&scanned) {
                hits.push(format!(
                    "{}:{}: `{}` in {line}",
                    path.display(),
                    n + 1,
                    &c[2]
                ));
            }
        }
        sources.push(text);
    }
    assert!(
        hits.is_empty(),
        "stage 1 must not know these words:\n{}",
        hits.join("\n")
    );

    // An exemption nobody needs any more is an exemption that quietly widens the
    // hole, so each one has to still be in use.
    for id in UPSTREAM {
        assert!(
            sources.iter().any(|t| t.contains(id)),
            "`{id}` is exempted but no longer appears in src/; drop it from UPSTREAM"
        );
    }

    // ★ The guard's own reach, asserted rather than assumed. A denylist passes
    // trivially when it happens to name nothing the next author would write, and
    // there is no way to tell the two cases apart from a green run — so state what
    // it catches, and state the hole.
    let caught = |line: &str| words.is_match(line);
    assert!(caught("let requirement = ..."), "a layered format's unit");
    assert!(caught("// one Scenario per branch"), "case is folded");
    assert!(
        caught("/// the document SHALL name it"),
        "RFC 2119 strength"
    );
    // ★ The case `\b` silently misses: format knowledge arriving as an identifier.
    assert!(caught("fn render_delta(&self)"), "snake_case is reached");
    assert!(
        caught("let parse_status = ..."),
        "and so is the older family"
    );
    // A longer word containing one is a different token; like `tree_guard.rs`, this
    // is a guard against a word coming back, not against someone hiding one.
    assert!(!caught("requirementless"));
    // And the hole: ikigai's own word cannot be denied, so nothing here would stop
    // `capability` from becoming format knowledge in `src/`.
    assert!(!caught("the caller's capability"));
}

/// Every `md:` term the lift or a mapping can write is defined in the served
/// vocabulary — the check that keeps `vocabulary.ttl` from drifting behind the code.
#[test]
fn every_term_the_lift_writes_is_in_the_vocabulary() {
    let vocab = ikigai_markdown::VOCABULARY;
    let profile = Mapping::parse(
        r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
           <#m> a md:Mapping ; md:split [ md:key "list" ] ;
             md:citation [ md:name "k" ; md:pattern "Key" ] ."#,
        "urn:test:m",
    )
    .unwrap()
    .profile;
    let mut text = SAMPLE.to_string();
    text.push_str("\n![alt](img.png)\n");
    let mut terms = std::collections::BTreeSet::new();
    for q in lift(&text, &doc("s.md"), &profile) {
        terms.insert(q.predicate.as_str().to_string());
        if let oxigraph::model::Term::NamedNode(n) = &q.object {
            terms.insert(n.as_str().to_string());
        }
    }
    // The mapping's own terms, as a mapping author writes them.
    for t in [
        "Mapping",
        "split",
        "key",
        "delimiter",
        "citation",
        "name",
        "pattern",
        "construct",
    ] {
        terms.insert(format!("{}{t}", ikigai_markdown::MD));
    }
    let missing: Vec<String> = terms
        .iter()
        .filter_map(|t| t.strip_prefix(ikigai_markdown::MD))
        .filter(|local| !vocab.contains(&format!("md:{local} a ")))
        .map(str::to_string)
        .collect();
    assert!(missing.is_empty(), "undefined md: terms: {missing:?}");
    assert!(terms.contains(&format!("{}Image", ikigai_markdown::MD)));
}
