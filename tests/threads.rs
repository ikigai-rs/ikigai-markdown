//! ★ Two golden threads. A lift is a pure function of (document bytes, mapping
//! bytes), so a cached lift must go stale when EITHER changes — and editing the
//! mapping must re-lift without rebuilding anything. This is the failure the
//! module's design note warns about: get it wrong and every derived graph is
//! silently stale the moment someone tunes the mapping.
//!
//! The document and the mapping are served the way `ikigai-fs` serves a watched
//! file: `.cacheable()` and `depends_on` their own IRI. The cut is the store's (or a
//! watcher's) job; this module holds nothing and watches nothing.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use ikigai_core::{
    ArgRef, Capability, Description, Exact, FnEndpoint, Invocation, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};

const DOC: &str = "urn:file:notes/a.md";
const MAPPING: &str = "urn:file:notes.mapping.ttl";

const TITLE_AS_LABEL: &str = r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
<#m> a md:Mapping ; md:construct """
PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { ?d <urn:ex:label> ?t } WHERE { ?h a md:Heading ; md:document ?d ; md:level 1 ; md:text ?t }""" ."#;

const TITLE_AS_NAME: &str = r#"@prefix md: <https://ikigai-rs.dev/ns/md#> .
<#m> a md:Mapping ; md:construct """
PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { ?d <urn:ex:name> ?t } WHERE { ?h a md:Heading ; md:document ?d ; md:level 1 ; md:text ?t }""" ."#;

fn watched(id: &'static str, media: &'static str, store: Arc<RwLock<String>>) -> FnEndpoint {
    FnEndpoint::new(id, move |inv: &Invocation<'_>| {
        let bytes = store.read().unwrap().as_bytes().to_vec();
        Ok(Representation::new(ReprType::new(media), bytes)
            .cacheable()
            .depends_on(inv.request.target.as_str()))
    })
    .with_description(
        Description::new(id)
            .title("Watched file")
            .summary("a file served under its own golden thread")
            .verb(Verb::Source)
            .output(media),
    )
}

fn lift_request() -> Request {
    Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(DOC.into()))
        .with_arg("mapping", ArgRef::Inline(MAPPING.into()))
}

fn lift(kernel: &Kernel) -> String {
    let repr = futures::executor::block_on(kernel.issue(lift_request(), &Capability::root()))
        .expect("lift");
    String::from_utf8(repr.bytes).unwrap()
}

#[test]
fn editing_the_mapping_relifts_without_a_rebuild() {
    let doc = Arc::new(RwLock::new("# First Title\n".to_string()));
    let mapping = Arc::new(RwLock::new(TITLE_AS_LABEL.to_string()));
    let space = ikigai_markdown::space()
        .bind(
            Exact::new(DOC),
            watched("doc", "text/markdown", Arc::clone(&doc)),
        )
        .bind(
            Exact::new(MAPPING),
            watched("mapping", "text/turtle", Arc::clone(&mapping)),
        );
    let kernel = Kernel::new(Arc::new(space));

    let first = lift(&kernel);
    assert!(first.contains("<urn:ex:label> \"First Title\""), "{first}");
    assert!(
        kernel.is_cached(&lift_request(), &Capability::root()),
        "over watched inputs the lift is cached"
    );
    let repr =
        futures::executor::block_on(kernel.issue(lift_request(), &Capability::root())).unwrap();
    let threads: BTreeSet<String> = repr.threads().iter().map(|t| t.to_string()).collect();
    for thread in [DOC, MAPPING] {
        assert!(
            threads.contains(thread),
            "the lift carries `{thread}`: {threads:?}"
        );
    }

    // The mapping changes on disk. Nothing notices until its thread is cut.
    *mapping.write().unwrap() = TITLE_AS_NAME.to_string();
    assert_eq!(lift(&kernel), first, "no cut: served from the cache");

    kernel.cut(MAPPING);
    let relifted = lift(&kernel);
    assert!(
        relifted.contains("<urn:ex:name> \"First Title\""),
        "{relifted}"
    );
    assert!(!relifted.contains("<urn:ex:label>"), "{relifted}");

    // And the other thread: the document changes and its cut re-lifts too.
    *doc.write().unwrap() = "# Second Title\n".to_string();
    assert_eq!(lift(&kernel), relifted, "no cut: served from the cache");
    kernel.cut(DOC);
    let again = lift(&kernel);
    assert!(again.contains("<urn:ex:name> \"Second Title\""), "{again}");
}

#[test]
fn a_live_input_makes_the_lift_uncacheable() {
    // The least cacheable dependency wins: a mapping served live (no thread, not
    // cacheable) must drag the lift down with it rather than pin a stale graph.
    let live = FnEndpoint::new("live-mapping", |_: &Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new("text/turtle"),
            TITLE_AS_LABEL.as_bytes().to_vec(),
        ))
    })
    .with_description(
        Description::new("live-mapping")
            .title("Live mapping")
            .summary("a mapping served uncacheable")
            .verb(Verb::Source)
            .output("text/turtle"),
    );
    let doc = Arc::new(RwLock::new("# T\n".to_string()));
    let space = ikigai_markdown::space()
        .bind(Exact::new(DOC), watched("doc", "text/markdown", doc))
        .bind(Exact::new(MAPPING), live);
    let kernel = Kernel::new(Arc::new(space));
    lift(&kernel);
    assert!(!kernel.is_cached(&lift_request(), &Capability::root()));
}
