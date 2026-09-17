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
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(home)));
    let report = Suite::new()
        // The module DEFINES this namespace: `urn:markdown:vocabulary` serves it.
        .namespace(ikigai_markdown::MD)
        .pure(ikigai_markdown::VOCABULARY_ID)
        .cacheable(ikigai_markdown::VOCABULARY_ID)
        .pure(ikigai_markdown::LIFT_ID)
        .cacheable(ikigai_markdown::LIFT_ID)
        .cacheable(ikigai_markdown::MAPPING_ID)
        .fixture(Fixture::new(ikigai_markdown::MAPPING_ID, Verb::Source).binding("name", "example"))
        .fixture(
            Fixture::new(ikigai_markdown::LIFT_ID, Verb::Source)
                .arg("src", "# A title\n\n- an item\n")
                .arg("mapping", MAPPING)
                .arg("path", "notes/a.md"),
        )
        .run_blocking(&kernel);
    eprintln!("{report}");
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.endpoints, 3, "{report}");
}
