//! The example mapping (`mappings/example`) and its queries, run over the invented
//! decision records in `tests/fixtures/example`. Each test pins one thing a mapping
//! author relies on: two metadata shapes landing on the same predicates, a delimited
//! field split by the lift profile, prose citations, and a dangling reference found
//! by a query.
//!
//! The mapping's golden thread, through a named mapping in a config home, is in
//! `tests/named.rs`.

mod common;

use common::*;

const DR: &str = "PREFIX dr: <http://example.org/decisions#>\n";

#[test]
fn both_metadata_shapes_land_on_the_same_predicates() {
    // 0001 and 0003 write frontmatter; 0002 writes a `## Record` bullet list. The
    // query cannot tell which, and neither can anything built on it.
    let store = fixture_store();
    assert_eq!(
        select(&store, &query("decisions.rq")),
        vec![
            row(&["0001", "Use Turtle for mappings", "accepted", "2026-01-12"]),
            row(&["0002", "Cache every lift", "accepted", "2026-02-03"]),
            row(&["0003", "Find mappings by name", "proposed", "2026-03-01"]),
        ]
    );
}

#[test]
fn dates_are_typed_from_either_shape() {
    let store = fixture_store();
    let rows = select(
        &store,
        &format!(
            "{DR}SELECT ?e (STR(DATATYPE(?d)) AS ?type) WHERE {{ ?e dr:date ?d }} ORDER BY ?e"
        ),
    );
    let xsd_date = "http://www.w3.org/2001/XMLSchema#date";
    assert_eq!(
        rows,
        vec![
            row(&["urn:example:decision:0001", xsd_date]),
            row(&["urn:example:decision:0002", xsd_date]),
            row(&["urn:example:decision:0003", xsd_date]),
        ]
    );
}

#[test]
fn a_delimited_field_is_split_by_the_lift_profile() {
    // `related: 0001, 0002, 0009` is ONE frontmatter string; the profile's split makes
    // it three edges, including one to a decision that does not exist.
    let store = fixture_store();
    assert_eq!(
        select(
            &store,
            &format!("{DR}SELECT ?from ?to WHERE {{ ?from dr:related ?to }} ORDER BY ?from ?to"),
        ),
        vec![
            row(&["urn:example:decision:0001", "urn:example:decision:0002"]),
            row(&["urn:example:decision:0003", "urn:example:decision:0001"]),
            row(&["urn:example:decision:0003", "urn:example:decision:0002"]),
            row(&["urn:example:decision:0003", "urn:example:decision:0009"]),
        ]
    );

    // And without the split there is no way to get those rows: the same mapping
    // minus its md:split yields no `related` edges at all.
    let unsplit = std::fs::read_to_string(mapping_path())
        .unwrap()
        .replace("md:split <urn:example:mapping:split:related> ;", "");
    let store = load(&corpus(&[]), &unsplit);
    let rows = select(
        &store,
        &format!("{DR}SELECT ?from ?to WHERE {{ ?from dr:related ?to }}"),
    );
    // 0001's single-valued `related: 0002` is not split either, so nothing at all.
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn prose_citations_become_edges() {
    let store = fixture_store();
    assert_eq!(
        select(
            &store,
            &format!("{DR}SELECT ?from ?to WHERE {{ ?from dr:cites ?to }} ORDER BY ?from ?to"),
        ),
        vec![
            row(&["urn:example:decision:0002", "urn:example:decision:0001"]),
            row(&["urn:example:decision:0002", "urn:example:decision:0007"]),
            row(&["urn:example:decision:0003", "urn:example:decision:0002"]),
        ]
    );
}

#[test]
fn the_dangling_query_finds_the_planted_references() {
    let store = fixture_store();
    assert_eq!(
        select(&store, &query("dangling.rq")),
        vec![
            row(&["0002", "cites", "0007"]),
            // Visible only because the split made `0009` a token.
            row(&["0003", "related", "0009"]),
        ]
    );

    // Add the missing decisions and nothing dangles: the query is not just
    // returning a fixed answer.
    let store = load(
        &corpus(&[
            (
                "decisions/0007-measure-lifts.md",
                "---\nstatus: accepted\ndate: 2026-02-10\n---\n\n# Measure lifts\n",
            ),
            (
                "decisions/0009-later.md",
                "# Later\n\n## Record\n\n- **Status**: proposed\n",
            ),
        ]),
        &std::fs::read_to_string(mapping_path()).unwrap(),
    );
    assert!(select(&store, &query("dangling.rq")).is_empty());
}
