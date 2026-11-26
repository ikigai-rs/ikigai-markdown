//! The dialect mappings: `mappings/lectern` (Lectern Markdown slide decks) and
//! `mappings/mdbook` (an mdBook's source), with the course directives both share.
//!
//! Each fixture lifts to a pinned graph (`tests/fixtures/{dialect}/expected.nt`), each
//! mapping is found by name in a config home, and the directives lift with their fields,
//! the same in either dialect. Run with `UPDATE_EXPECTED=1` to rewrite the pinned graphs
//! after an intended change, and read the diff before committing it.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use ikigai_markdown::lint::check_mapping;
use ikigai_markdown::MappingHome;
use oxigraph::model::{NamedOrBlankNode, QuadRef};
use oxigraph::store::Store;

const LT: &str = "PREFIX lt: <https://ikigai-rs.dev/ns/lectern#>\n";
const MB: &str = "PREFIX mb: <https://ikigai-rs.dev/ns/mdbook#>\n";
const LEARN: &str = "PREFIX learn: <https://ikigai-rs.dev/ns/learn#>\n";

fn dialect_dir(name: &str) -> PathBuf {
    repo().join("mappings").join(name)
}

fn dialect(name: &str) -> String {
    std::fs::read_to_string(dialect_dir(name).join("mapping.ttl")).unwrap()
}

fn fixture(name: &str) -> Vec<(String, String)> {
    tree(&repo().join("tests/fixtures").join(name))
}

/// The domain graph: every triple whose subject is not a structural node, as sorted
/// N-Triples. Structural nodes (`urn:markdown:doc:…`) are the generic lift's, pinned in
/// `tests/lift.rs`; what is left is what the mapping wrote.
fn domain(store: &Store) -> String {
    let mut lines: Vec<String> = store
        .iter()
        .map(|q| q.unwrap())
        .filter(|q| match &q.subject {
            NamedOrBlankNode::NamedNode(n) => !n.as_str().starts_with("urn:markdown:doc:"),
            NamedOrBlankNode::BlankNode(_) => true,
        })
        .map(|q| {
            let q = QuadRef::from(&q);
            format!("{} {} {} .", q.subject, q.predicate, q.object)
        })
        .collect();
    lines.sort();
    lines.dedup();
    lines.join("\n") + "\n"
}

fn pinned(name: &str, actual: &str) {
    let path = repo().join("tests/fixtures").join(name).join("expected.nt");
    if std::env::var_os("UPDATE_EXPECTED").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "{name}: the lift no longer matches {}; rerun with UPDATE_EXPECTED=1 and read the \
         diff\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

fn lectern(docs: &[(String, String)]) -> Store {
    load(docs, &dialect("lectern"))
}

fn mdbook(docs: &[(String, String)]) -> Store {
    load(docs, &dialect("mdbook"))
}

fn doc(path: &str, text: &str) -> Vec<(String, String)> {
    vec![(path.to_string(), text.to_string())]
}

// --- the fixtures, pinned ----------------------------------------------------------------

#[test]
fn the_lectern_fixture_deck_lifts_to_the_expected_graph() {
    let store = lectern(&fixture("lectern"));
    let graph = domain(&store);
    assert!(
        !graph.contains("_:"),
        "the lift wrote a blank node:\n{graph}"
    );
    pinned("lectern", &graph);
}

#[test]
fn the_mdbook_fixture_book_lifts_to_the_expected_graph() {
    let store = mdbook(&fixture("mdbook"));
    let graph = domain(&store);
    assert!(
        !graph.contains("_:"),
        "the lift wrote a blank node:\n{graph}"
    );
    pinned("mdbook", &graph);
}

#[test]
fn a_lift_is_the_same_graph_every_time() {
    let docs = fixture("lectern");
    assert_eq!(domain(&lectern(&docs)), domain(&lectern(&docs)));
    let docs = fixture("mdbook");
    assert_eq!(domain(&mdbook(&docs)), domain(&mdbook(&docs)));
}

#[test]
fn both_mappings_check_clean() {
    for name in ["lectern", "mdbook"] {
        let text = check_mapping(&dialect(name), "urn:test:mapping").to_text();
        assert!(
            text.ends_with("  result: 0 errors, 0 warnings, 0 notes\n"),
            "{name}:\n{text}"
        );
    }
}

// --- found by name ----------------------------------------------------------------------

#[test]
fn each_mapping_is_found_by_name_and_lifts_like_the_file() {
    let scratch = Scratch::new("dialects");
    scratch.install_dir("lectern", &dialect_dir("lectern"));
    scratch.install_dir("mdbook", &dialect_dir("mdbook"));
    let home = Arc::new(MappingHome::new(Some(scratch.path().to_path_buf())));
    for (name, by_file) in [
        ("lectern", lectern(&fixture("lectern"))),
        ("mdbook", mdbook(&fixture("mdbook"))),
    ] {
        let by_name = load_in(
            mounted(&home),
            &fixture(name),
            &format!("urn:markdown:mapping:{name}"),
        );
        let graph = domain(&by_name);
        assert!(graph.lines().count() > 20, "{name}: {graph}");
        assert_eq!(graph, domain(&by_file), "{name}");
    }
}

// --- lectern: slides ---------------------------------------------------------------------

/// (number, title) of every slide in `path`, in order.
fn slides(store: &Store, path: &str) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LT}SELECT ?n ?title WHERE {{ ?deck lt:path \"{path}\" ; lt:slide ?s . \
             ?s lt:number ?n OPTIONAL {{ ?s lt:title ?title }} }} ORDER BY ?n"
        ),
    )
}

/// The `md:text` of every block on slide `n`, in document order.
fn on_slide(store: &Store, n: u32) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LT}PREFIX md: <https://ikigai-rs.dev/ns/md#>\n\
             SELECT ?text WHERE {{ ?s lt:number {n} ; lt:block ?b . ?b md:order ?o \
             OPTIONAL {{ ?b md:text ?text }} }} ORDER BY ?o"
        ),
    )
}

#[test]
fn slides_split_as_lecterns_splitter_splits_them() {
    let store = lectern(&fixture("lectern"));
    assert_eq!(
        slides(&store, "deck.md"),
        vec![
            row(&["1", "Graphs from text"]),
            row(&["2", "Triples"]),
            // Two includes and nothing else: no heading, so no title.
            row(&["3", ""]),
            // Empty, between two separators, and still counted: an include's range
            // counts slides the same way.
            row(&["4", ""]),
            row(&["5", "Practice"]),
        ]
    );
    assert_eq!(
        slides(&store, "partials/basics.md"),
        vec![
            row(&["1", "What a graph is"]),
            row(&["2", "What a node is"]),
            row(&["3", "What an edge is"]),
            row(&["4", "Why it matters"]),
        ]
    );
}

#[test]
fn a_separator_in_fenced_code_is_code() {
    let deck = "# One\n\n```\n---\n```\n\n~~~~\n---\n~~~~\n\n---\n\n# Two\n";
    let store = lectern(&doc("d.md", deck));
    assert_eq!(
        slides(&store, "d.md"),
        vec![row(&["1", "One"]), row(&["2", "Two"])]
    );
    assert_eq!(
        on_slide(&store, 1),
        vec![row(&["One"]), row(&["---\n"]), row(&["---\n"])]
    );
}

#[test]
fn every_line_lectern_reads_as_a_separator_is_one() {
    // Straight after a paragraph line `---` is a setext underline to CommonMark, and
    // indented or with trailing blanks it is still `---` once stripped; lectern splits
    // at all three. CRLF line endings too. A leading separator makes an empty slide 1 and
    // a trailing one an empty last slide; lectern keeps both, and counts them.
    let deck = "\n---\n# A\ntext\n---\n# B\n   ---  \n# C\r\n---\r\n# D\n---\n";
    let store = lectern(&doc("d.md", deck));
    assert_eq!(
        slides(&store, "d.md"),
        vec![
            row(&["1", ""]),
            row(&["2", "A"]),
            row(&["3", "B"]),
            row(&["4", "C"]),
            row(&["5", "D"]),
            row(&["6", ""]),
        ]
    );
}

#[test]
fn a_decks_frontmatter_is_not_a_slide() {
    // lectern reads a YAML block on line 1 as frontmatter and ignores it; its delimiters
    // are not separators.
    let deck = "---\ntitle: ignored\n---\n# A\n\n---\n\n# B\n";
    let store = lectern(&doc("d.md", deck));
    assert_eq!(
        slides(&store, "d.md"),
        vec![row(&["1", "A"]), row(&["2", "B"])]
    );
}

#[test]
fn a_deck_with_no_separator_is_one_slide() {
    let store = lectern(&doc("d.md", "# Only\n\nOne slide.\n"));
    assert_eq!(slides(&store, "d.md"), vec![row(&["1", "Only"])]);
    let store = lectern(&doc("d.md", ""));
    assert_eq!(slides(&store, "d.md"), vec![row(&["1", ""])]);
}

#[test]
fn a_slide_iri_is_its_deck_path_and_number() {
    let store = lectern(&doc("talks/intro deck.md", "# A\n\n---\n\n# B\n"));
    assert_eq!(
        select(
            &store,
            &format!("{LT}SELECT ?s WHERE {{ ?s a lt:Slide }} ORDER BY ?s")
        ),
        vec![
            row(&["urn:lectern:slide:path:talks%2Fintro%20deck.md:number:1"]),
            row(&["urn:lectern:slide:path:talks%2Fintro%20deck.md:number:2"]),
        ]
    );
}

// --- lectern: notes ----------------------------------------------------------------------

/// (slide, index, kind, the text of each block in the note) for every note.
fn notes(store: &Store) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LT}PREFIX md: <https://ikigai-rs.dev/ns/md#>\n\
             SELECT ?n ?k ?kind ?text WHERE {{ ?note lt:slide ?s ; lt:index ?k ; lt:kind ?kind . \
             ?s lt:number ?n OPTIONAL {{ ?note lt:block ?b . ?b md:text ?text ; md:order ?o }} }} \
             ORDER BY ?n ?k ?o"
        ),
    )
}

#[test]
fn speaker_and_presenter_notes_lift_with_what_they_hold() {
    let store = lectern(&fixture("lectern"));
    assert_eq!(
        notes(&store)
            .into_iter()
            .filter(|r| !r[0].is_empty())
            .collect::<Vec<_>>(),
        vec![
            row(&[
                "1",
                "1",
                "speaker",
                "Welcome everyone, and say what the hour covers."
            ]),
            row(&["2", "1", "presenter", "Pause here for questions."]),
            row(&["5", "1", "speaker", "Hand out the exercise."]),
            row(&[
                "5",
                "2",
                "presenter",
                "Remind people that the check script runs locally."
            ]),
        ]
    );
}

#[test]
fn an_unclosed_note_ends_with_its_slide_and_an_unknown_category_is_a_speaker_note() {
    let deck = "# A\n\n<!-- notes:aside -->\nFirst.\n\nSecond.\n\n---\n\n# B\n\nOn the slide.\n";
    let store = lectern(&doc("d.md", deck));
    assert_eq!(
        notes(&store),
        vec![
            row(&["1", "1", "speaker", "First."]),
            row(&["1", "1", "speaker", "Second."]),
        ]
    );
}

#[test]
fn a_note_marker_in_fenced_code_is_code() {
    let deck = "# A\n\n```html\n<!-- notes -->\nshown, not a note\n<!-- /notes -->\n```\n";
    let store = lectern(&doc("d.md", deck));
    assert!(notes(&store).is_empty());
}

// --- lectern: includes ---------------------------------------------------------------------

/// (target, ranges as written, part, first, last) for every include, in order.
fn includes(store: &Store) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LT}SELECT ?target ?ranges ?part ?first ?last WHERE {{ \
             ?i a lt:Include ; lt:slide ?s ; lt:index ?k ; lt:target ?target . ?s lt:number ?n \
             OPTIONAL {{ ?i lt:ranges ?ranges }} \
             OPTIONAL {{ ?i lt:range ?r . ?r lt:index ?part \
               OPTIONAL {{ ?r lt:first ?first }} OPTIONAL {{ ?r lt:last ?last }} }} \
             }} ORDER BY ?n ?k ?part"
        ),
    )
}

#[test]
fn an_include_lifts_its_target_and_each_part_of_its_ranges() {
    let store = lectern(&fixture("lectern"));
    assert_eq!(
        includes(&store),
        vec![
            row(&["partials/basics.md", "1-2, 4-", "1", "1", "2"]),
            row(&["partials/basics.md", "1-2, 4-", "2", "4", ""]),
            row(&["partials/basics.md", "", "", "", ""]),
        ]
    );
}

#[test]
fn every_range_form_lectern_reads() {
    let deck = "<!-- include: a.md#2 -->\n\n<!-- include: b.md#-3 -->\n\n\
                <!-- include: c.md# 1 - 3 , 14 -->\n\n<!-- include: d.md# -->\n";
    let store = lectern(&doc("d.md", deck));
    assert_eq!(
        includes(&store),
        vec![
            row(&["a.md", "2", "1", "2", "2"]),
            row(&["b.md", "-3", "1", "1", "3"]),
            row(&["c.md", "1 - 3 , 14", "1", "1", "3"]),
            row(&["c.md", "1 - 3 , 14", "2", "14", "14"]),
            // `path#` with nothing after it is the whole file, as lectern reads it.
            row(&["d.md", "", "", "", ""]),
        ]
    );
}

// --- the course directives, in both dialects ---------------------------------------------

/// Every question's fields, one row per answer (or none).
fn questions(store: &Store) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LEARN}SELECT ?q ?name ?unit ?kind ?prompt ?answer ?check WHERE {{ \
             ?q a learn:Question ; learn:name ?name ; learn:unit ?unit ; learn:kind ?kind ; \
             learn:prompt ?prompt \
             OPTIONAL {{ ?q learn:answer ?answer }} OPTIONAL {{ ?q learn:check ?check }} }} \
             ORDER BY ?q ?answer"
        ),
    )
}

fn choices(store: &Store, question: &str) -> Vec<Vec<String>> {
    select(
        store,
        &format!(
            "{LEARN}SELECT ?i ?text ?correct WHERE {{ <{question}> learn:choice ?c . \
             ?c learn:index ?i ; learn:text ?text ; learn:correct ?correct }} ORDER BY ?i"
        ),
    )
}

#[test]
fn questions_lift_with_their_fields() {
    let store = lectern(&fixture("lectern"));
    let q = |n: &str| format!("urn:learn:question:unit:rdf-basics:name:{n}");
    let unit = "urn:learn:unit:name:rdf-basics";
    assert_eq!(
        questions(&store),
        vec![
            row(&[
                &q("count-triples"),
                "count-triples",
                unit,
                "exact",
                "How many triples does the sample graph hold?",
                "12",
                ""
            ]),
            row(&[
                &q("count-triples"),
                "count-triples",
                unit,
                "exact",
                "How many triples does the sample graph hold?",
                "twelve",
                ""
            ]),
            row(&[
                &q("load-check"),
                "load-check",
                unit,
                "check",
                "Load the sample graph and print its size.",
                "",
                "exercises/load-a-graph/check.sh"
            ]),
            row(&[
                &q("triple-parts"),
                "triple-parts",
                unit,
                "choice",
                "Which three parts make a triple?",
                "",
                ""
            ]),
            row(&[
                &q("why-graphs"),
                "why-graphs",
                unit,
                "short",
                "In one sentence, why lift notes into a graph?",
                "So that what the notes say can be queried and joined.",
                ""
            ]),
        ]
    );
    assert_eq!(
        choices(&store, &q("triple-parts")),
        vec![
            row(&["1", "subject, predicate, object", "true"]),
            row(&["2", "subject, verb, object", "false"]),
            row(&["3", "node, edge, label", "false"]),
        ]
    );
    // Each question is on the slide it sits on.
    assert_eq!(
        select(
            &store,
            &format!(
                "{LT}{LEARN}SELECT ?path ?n ?q WHERE {{ ?s learn:question ?q ; lt:number ?n ; \
                 lt:deck ?d . ?d lt:path ?path }} ORDER BY ?path ?n ?q"
            )
        ),
        vec![
            row(&["deck.md", "2", &q("triple-parts")]),
            row(&["deck.md", "5", &q("count-triples")]),
            row(&["deck.md", "5", &q("load-check")]),
            row(&["partials/basics.md", "4", &q("why-graphs")]),
        ]
    );
}

#[test]
fn exercise_references_lift_with_their_fields() {
    let store = lectern(&fixture("lectern"));
    assert_eq!(
        select(
            &store,
            &format!(
                "{LT}{LEARN}SELECT ?n ?e ?name ?unit ?path ?title WHERE {{ \
                 ?s learn:exercise ?e ; lt:number ?n . \
                 ?e a learn:Exercise ; learn:name ?name ; learn:unit ?unit \
                 OPTIONAL {{ ?e learn:path ?path }} OPTIONAL {{ ?e learn:title ?title }} }}"
            )
        ),
        vec![row(&[
            "5",
            "urn:learn:exercise:unit:rdf-basics:name:load-a-graph",
            "load-a-graph",
            "urn:learn:unit:name:rdf-basics",
            "exercises/load-a-graph",
            "Load a graph and count its triples",
        ])]
    );
    let store = mdbook(&fixture("mdbook"));
    assert_eq!(
        select(
            &store,
            &format!(
                "{LEARN}SELECT ?c ?e ?path WHERE {{ ?c learn:exercise ?e . \
                 ?e learn:path ?path }}"
            )
        ),
        vec![row(&[
            "urn:mdbook:chapter:path:src%2Fgraphs%2Ftriples.md",
            "urn:learn:exercise:unit:rdf-basics:name:load-a-graph",
            "exercises/load-a-graph",
        ])]
    );
}

/// The directive text both dialects read, placed in a deck and in a chapter.
const DIRECTIVES: &str = "<!-- question
name: pick-one
unit: shared-unit
kind: choice
prompt: Which of these is a fruit?
choice: [ ] carrot
choice: [X] apple
choice: [x] pear
-->

<!-- exercise
name: sort-produce
unit: shared-unit
title: Sort the produce
-->
";

#[test]
fn a_directive_lifts_the_same_in_either_dialect() {
    let learn_only = |store: &Store| -> String {
        domain(store)
            .lines()
            .filter(|l| l.starts_with("<urn:learn:"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let deck = lectern(&doc("deck.md", &format!("# Produce\n\n{DIRECTIVES}")));
    let chapter = mdbook(&doc(
        "src/produce.md",
        &format!("# Produce\n\n{DIRECTIVES}"),
    ));
    let graph = learn_only(&deck);
    assert_eq!(graph, learn_only(&chapter));
    assert!(graph.contains("<urn:learn:question:unit:shared-unit:name:pick-one>"));
    assert_eq!(
        choices(&deck, "urn:learn:question:unit:shared-unit:name:pick-one"),
        vec![
            row(&["1", "carrot", "false"]),
            row(&["2", "apple", "true"]),
            row(&["3", "pear", "true"]),
        ]
    );
}

#[test]
fn a_directive_in_fenced_code_or_without_its_keys_is_not_lifted() {
    let text = "# A\n\n```markdown\n<!-- question\nname: shown\nunit: u\nkind: short\n\
                prompt: An example.\n-->\n```\n\n<!-- question\nunit: u\nkind: short\n\
                prompt: No name.\n-->\n\n<!-- exercise\nname: no-unit\n-->\n";
    for store in [lectern(&doc("d.md", text)), mdbook(&doc("src/d.md", text))] {
        assert!(questions(&store).is_empty());
        assert!(select(
            &store,
            &format!("{LEARN}SELECT ?e WHERE {{ ?e a learn:Exercise }}")
        )
        .is_empty());
    }
}

// --- mdbook: chapters and the summary ------------------------------------------------------

#[test]
fn the_summary_lists_chapters_in_order_with_their_nesting() {
    let store = mdbook(&fixture("mdbook"));
    let ch = |p: &str| format!("urn:mdbook:chapter:path:{}", p.replace('/', "%2F"));
    assert_eq!(
        select(
            &store,
            &format!(
                "{MB}SELECT ?position ?c ?listed ?numbered ?parent ?title WHERE {{ \
                 ?c mb:position ?position ; mb:listedAs ?listed ; mb:numbered ?numbered \
                 OPTIONAL {{ ?c mb:parent ?parent }} OPTIONAL {{ ?c mb:title ?title }} }} \
                 ORDER BY ?position"
            )
        ),
        vec![
            row(&[
                "1",
                &ch("src/intro.md"),
                "Introduction",
                "false",
                "",
                "Introduction"
            ]),
            row(&[
                "2",
                &ch("src/graphs/triples.md"),
                "Triples",
                "true",
                "",
                "Triples"
            ]),
            row(&[
                "3",
                &ch("src/graphs/literals.md"),
                "Literals",
                "true",
                &ch("src/graphs/triples.md"),
                "Literals"
            ]),
            // Listed, with no file behind it in the fixture: a title-less chapter.
            row(&["4", &ch("src/graphs/queries.md"), "Queries", "true", "", ""]),
        ]
    );
    // Every file but the summary is a chapter, listed or not.
    assert_eq!(
        select(
            &store,
            &format!("{MB}SELECT ?path WHERE {{ ?c mb:path ?path }} ORDER BY ?path")
        ),
        vec![
            row(&["src/graphs/literals.md"]),
            row(&["src/graphs/triples.md"]),
            row(&["src/intro.md"]),
        ]
    );
}
