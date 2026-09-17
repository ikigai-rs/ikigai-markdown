//! Named mappings in the config home: `urn:markdown:mapping:{name}` serves
//! `<home>/markdown/mappings/{name}/mapping.ttl`.
//!
//! Every test here owns its config home (a scratch directory), so nothing reads the
//! developer's real `~/.config/ikigai`.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_core::{ArgRef, Capability, Error, Iri, Kernel, Request, Verb};
use ikigai_markdown::MappingHome;

fn home(scratch: &Scratch) -> Arc<MappingHome> {
    Arc::new(MappingHome::new(Some(scratch.path().to_path_buf())))
}

fn source(kernel: &Kernel, request: Request) -> Result<String, Error> {
    futures::executor::block_on(kernel.issue(request, &Capability::root()))
        .map(|r| String::from_utf8(r.bytes).unwrap())
}

/// A document served the way a watched file is: cacheable, under its own thread. The
/// lift is only as cacheable as its least cacheable input, so the mapping's half of
/// the thread is only observable when the document has one too.
fn watched(id: &'static str, text: &str) -> ikigai_core::FnEndpoint {
    let bytes = text.as_bytes().to_vec();
    ikigai_core::FnEndpoint::new(id, move |inv: &ikigai_core::Invocation<'_>| {
        Ok(ikigai_core::Representation::new(
            ikigai_core::ReprType::new("text/markdown"),
            bytes.clone(),
        )
        .cacheable()
        .depends_on(inv.request.target.as_str()))
    })
    .with_description(
        ikigai_core::Description::new(id)
            .title("Watched document")
            .summary("a document served under its own golden thread")
            .verb(Verb::Source)
            .output("text/markdown"),
    )
}

fn lift_request(src: &str, mapping: &str) -> Request {
    Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(src.as_bytes().to_vec()))
        .with_arg("path", ArgRef::Inline(b"decisions/0001-a.md".to_vec()))
        .with_arg("mapping", ArgRef::Inline(mapping.as_bytes().to_vec()))
}

#[test]
fn a_mapping_installed_by_name_lifts_like_the_file_itself() {
    let scratch = Scratch::new("by-name");
    scratch.install_example("example");
    let by_name = load_in(
        ikigai_markdown::space_with(home(&scratch)),
        &corpus(&[]),
        "urn:markdown:mapping:example",
    );
    let q = query("decisions.rq");
    assert_eq!(select(&by_name, &q), select(&fixture_store(), &q));
    assert_eq!(select(&by_name, &q).len(), 3);
}

#[test]
fn the_named_mapping_is_served_as_turtle_under_its_file_thread() {
    let scratch = Scratch::new("served");
    let dir = scratch.install_example("example");
    let home = home(&scratch);
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(home.clone())));
    let request = Request::new(
        Verb::Source,
        Iri::parse("urn:markdown:mapping:example").unwrap(),
    );
    let repr =
        futures::executor::block_on(kernel.issue(request, &Capability::root())).expect("source");
    assert_eq!(repr.repr_type.media_type, "text/turtle");
    assert_eq!(
        repr.bytes,
        std::fs::read(dir.join("mapping.ttl")).unwrap(),
        "the bytes of the installed file"
    );
    let thread = format!("urn:file:{}", dir.join("mapping.ttl").display());
    assert_eq!(home.thread("example").unwrap(), thread);
    assert!(
        repr.threads().iter().any(|t| t.to_string() == thread),
        "{:?}",
        repr.threads()
    );
    assert_eq!(home.queries_dir("example").unwrap(), dir.join("queries"));
}

/// ★ The mapping's golden thread, through the name: edit the installed file, cut its
/// thread the way a watcher would, and the lift re-runs under the new mapping with no
/// rebuild and no restart.
#[test]
fn editing_a_named_mapping_relifts_when_its_thread_is_cut() {
    let scratch = Scratch::new("thread");
    let dir = scratch.install_example("example");
    let home = home(&scratch);
    let doc = "---\nstatus: Accepted\n---\n\n# A\n";
    let space = ikigai_markdown::space_with(home.clone())
        .bind(ikigai_core::Exact::new("urn:test:doc"), watched("doc", doc));
    let kernel = Kernel::new(Arc::new(space));
    let request = || lift_request("urn:test:doc", "urn:markdown:mapping:example");

    let first = source(&kernel, request()).unwrap();
    assert!(first.contains("\"accepted\""), "{first}");
    assert!(
        kernel.is_cached(&request(), &Capability::root()),
        "a lift under a named mapping is cached"
    );

    // The file changes on disk: statuses are now upper-cased.
    let path = dir.join("mapping.ttl");
    let edited = std::fs::read_to_string(&path).unwrap().replace(
        r#"IF(?key = "status", LCASE(?value)"#,
        r#"IF(?key = "status", UCASE(?value)"#,
    );
    assert!(edited.contains("UCASE"), "the edit applied");
    std::fs::write(&path, edited).unwrap();
    assert_eq!(
        source(&kernel, request()).unwrap(),
        first,
        "no cut: served from the cache"
    );

    kernel.cut(home.thread("example").unwrap().as_str());
    let relifted = source(&kernel, request()).unwrap();
    assert!(relifted.contains("\"ACCEPTED\""), "{relifted}");
    assert!(!relifted.contains("\"accepted\""), "{relifted}");
}

#[test]
fn a_missing_name_is_not_found_and_names_the_path() {
    let scratch = Scratch::new("missing");
    scratch.install_example("example");
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(home(&scratch))));
    let expected = scratch
        .path()
        .join("markdown/mappings/absent/mapping.ttl")
        .display()
        .to_string();

    let direct = Request::new(
        Verb::Source,
        Iri::parse("urn:markdown:mapping:absent").unwrap(),
    );
    match source(&kernel, direct) {
        Err(Error::NotFound(msg)) => assert!(msg.contains(&expected), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }

    // Through the lift, the same typed error: no fallback to an unmapped lift.
    match source(
        &kernel,
        lift_request("# A\n", "urn:markdown:mapping:absent"),
    ) {
        Err(Error::NotFound(msg)) => assert!(msg.contains(&expected), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn no_config_home_is_not_found_at_resolution() {
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(Arc::new(
        MappingHome::new(None),
    ))));
    let request = Request::new(
        Verb::Source,
        Iri::parse("urn:markdown:mapping:example").unwrap(),
    );
    match source(&kernel, request) {
        Err(Error::NotFound(msg)) => assert!(msg.contains("no config home"), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn a_name_cannot_leave_the_mappings_directory() {
    let scratch = Scratch::new("escape");
    let home = MappingHome::new(Some(scratch.path().to_path_buf()));
    for bad in ["", ".", "..", "a/b", "../x", "a b"] {
        assert!(
            matches!(home.read(bad), Err(Error::InvalidArgument { .. })),
            "`{bad}` must be refused"
        );
    }
}
