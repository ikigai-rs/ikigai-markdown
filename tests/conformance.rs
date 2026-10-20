//! The module recipe as one test: `ikigai-conformance` walks every endpoint
//! [`ikigai_markdown::space_with`] binds and reports every violation at once.
//!
//! With `src` and `mapping` both given by value the lift is a pure function of its
//! arguments, so it is declared `pure` and `cacheable`. By reference it inherits its
//! inputs' threads instead — `tests/threads.rs` holds that half.

mod common;

use std::sync::Arc;

use common::Scratch;
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{Kernel, Verb};
use ikigai_markdown::MappingHome;

const MAPPING: &str = r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
<#m> a md:Mapping ; md:construct """
PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { ?d <http://purl.org/dc/terms/title> ?t } WHERE { ?h a md:Heading ; md:document ?d ; md:level 1 ; md:text ?t }""" ."#;

#[test]
fn conforms() {
    // A config home this test owns, with the example mapping installed in it, so the
    // named-mapping endpoint is walked against files the test wrote.
    let scratch = Scratch::new("conformance");
    scratch.install_example("example");
    let home = Arc::new(MappingHome::new(Some(scratch.path().to_path_buf())));
    // Built once and handed to both the kernel and the suite, so the space the walk
    // covers is the one SPACE-NAME checks.
    let space = Arc::new(ikigai_markdown::space_with(home));
    let kernel = Kernel::new(space.clone());
    let report = Suite::new()
        // Neither constructor is configuration-free (ledger #987), so neither names
        // itself and the host names the instance it mounts. `space_with(home)` is
        // built over the home it is handed; `space()` reads this machine's config
        // home from the environment while it is built (on native; the wasm build has
        // no named-mapping door and reads nothing).
        .host_named_space("ikigai_markdown::space_with(home)", space)
        .host_named_space("ikigai_markdown::space()", ikigai_markdown::space())
        // The module DEFINES this namespace: `urn:markdown:vocabulary` serves it.
        .namespace(ikigai_markdown::MD)
        .pure(ikigai_markdown::VOCABULARY_ID)
        .cacheable(ikigai_markdown::VOCABULARY_ID)
        .pure(ikigai_markdown::LIFT_ID)
        .cacheable(ikigai_markdown::LIFT_ID)
        .cacheable(ikigai_markdown::MAPPING_ID)
        .fixture(Fixture::new(ikigai_markdown::MAPPING_ID, Verb::Source).binding("name", "example"))
        // REQUIRED (0.8.0) drops `src` from this call and expects a typed refusal: with no
        // `content` either, the lift answers `InvalidArgument` naming `src`. `content` is
        // the by-value alternative, so `src` is "one of src/content" and the check cannot
        // spell that; it stays declared required because the engine pipes into the one
        // required input, and an all-optional lift could not be piped into at all.
        .fixture(
            Fixture::new(ikigai_markdown::LIFT_ID, Verb::Source)
                .arg("src", "# A title\n\n- an item\n")
                .arg("mapping", MAPPING)
                .arg("path", "notes/a.md"),
        )
        // The linter. Given a mapping by value, the mapping check and the expects report
        // are functions of their arguments alone.
        .pure(ikigai_markdown::LINT_MAPPING_ID)
        .cacheable(ikigai_markdown::LINT_MAPPING_ID)
        .fixture(
            Fixture::new(ikigai_markdown::LINT_MAPPING_ID, Verb::Source).arg("mapping", MAPPING),
        )
        .pure(ikigai_markdown::LINT_EXPECTS_ID)
        .cacheable(ikigai_markdown::LINT_EXPECTS_ID)
        .fixture(
            Fixture::new(ikigai_markdown::LINT_EXPECTS_ID, Verb::Source).arg("mapping", MAPPING),
        )
        // The snapshot and the corpus check read documents through the kernel, and keep
        // their threads; any served resource is a document, so the vocabulary stands in.
        .cacheable(ikigai_markdown::SNAPSHOT_ID)
        .fixture(
            Fixture::new(ikigai_markdown::SNAPSHOT_ID, Verb::Source)
                .arg("docs", ikigai_markdown::VOCABULARY_IRI)
                .arg("mapping", MAPPING),
        )
        .cacheable(ikigai_markdown::LINT_CORPUS_ID)
        .fixture(
            Fixture::new(ikigai_markdown::LINT_CORPUS_ID, Verb::Source)
                .arg("docs", ikigai_markdown::VOCABULARY_IRI)
                .arg("mapping", MAPPING),
        )
        .run_blocking(&kernel);
    eprintln!("{report}");
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.endpoints, 7, "{report}");
    // FAILURES is off unless a suite opts in (`check_failures()`, the next wave); it is the
    // only check allowed to be skipped.
    assert_eq!(
        report
            .checks
            .skipped()
            .map(|c| c.to_string())
            .collect::<Vec<_>>(),
        ["FAILURES"],
        "every check but FAILURES runs: {report}"
    );
}
