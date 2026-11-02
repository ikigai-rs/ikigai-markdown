//! Named mappings in the config home: `urn:markdown:mapping:{name}` serves
//! `<home>/markdown/mappings/{name}/mapping.ttl`, read BY REQUEST from the file mount
//! the host binds at `urn:markdown:file:{path}` (Hypothesis H, ledger #2384).
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
    source_as(kernel, request, &Capability::root())
}

fn source_as(kernel: &Kernel, request: Request, cap: &Capability) -> Result<String, Error> {
    futures::executor::block_on(kernel.issue(request, cap))
        .map(|r| String::from_utf8(r.bytes).unwrap())
}

fn named(name: &str) -> Request {
    Request::new(
        Verb::Source,
        Iri::parse(format!("urn:markdown:mapping:{name}")).unwrap(),
    )
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
        mounted(&home(&scratch)),
        &corpus(&[]),
        "urn:markdown:mapping:example",
    );
    let q = query("decisions.rq");
    assert_eq!(select(&by_name, &q), select(&fixture_store(), &q));
    assert_eq!(select(&by_name, &q).len(), 3);
}

/// The resource still answers the same: the bytes of the installed file, as Turtle, now
/// under the golden thread of the file's IRI in the host's mount.
#[test]
fn the_named_mapping_is_served_as_turtle_under_its_file_thread() {
    let scratch = Scratch::new("served");
    let dir = scratch.install_example("example");
    let home = home(&scratch);
    let kernel = Kernel::new(Arc::new(mounted(&home)));
    let repr = futures::executor::block_on(kernel.issue(named("example"), &Capability::root()))
        .expect("source");
    assert_eq!(repr.repr_type.media_type, "text/turtle");
    assert_eq!(
        repr.bytes,
        std::fs::read(dir.join("mapping.ttl")).unwrap(),
        "the bytes of the installed file"
    );
    let thread = "urn:markdown:file:mappings/example/mapping.ttl";
    assert_eq!(home.file("example").unwrap(), thread);
    assert!(
        repr.threads().iter().any(|t| t.to_string() == thread),
        "{:?}",
        repr.threads()
    );
    assert!(kernel.is_cached(&named("example"), &Capability::root()));
    assert_eq!(home.queries_dir("example").unwrap(), dir.join("queries"));
}

/// ★ The mapping's golden thread, through the name: edit the installed file, cut its
/// thread the way a watcher over the mount would, and the lift re-runs under the new
/// mapping with no rebuild and no restart.
#[test]
fn editing_a_named_mapping_relifts_when_its_thread_is_cut() {
    let scratch = Scratch::new("thread");
    let dir = scratch.install_example("example");
    let home = home(&scratch);
    let doc = "---\nstatus: Accepted\n---\n\n# A\n";
    let space = mounted(&home).bind(ikigai_core::Exact::new("urn:test:doc"), watched("doc", doc));
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

    kernel.cut(home.file("example").unwrap().as_str());
    let relifted = source(&kernel, request()).unwrap();
    assert!(relifted.contains("\"ACCEPTED\""), "{relifted}");
    assert!(!relifted.contains("\"accepted\""), "{relifted}");
}

/// A write through the kernel to the mapping's file cuts its thread by itself (the
/// kernel auto-cuts a write's own target), so the lift re-runs with no watcher at all.
#[test]
fn writing_the_mapping_through_the_kernel_relifts() {
    let scratch = Scratch::new("sink");
    let dir = scratch.install_example("example");
    let home = home(&scratch);
    let doc = "---\nstatus: Accepted\n---\n\n# A\n";
    let space = mounted(&home).bind(ikigai_core::Exact::new("urn:test:doc"), watched("doc", doc));
    let kernel = Kernel::new(Arc::new(space));
    let request = || lift_request("urn:test:doc", "urn:markdown:mapping:example");
    let first = source(&kernel, request()).unwrap();
    assert!(first.contains("\"accepted\""), "{first}");

    let edited = std::fs::read_to_string(dir.join("mapping.ttl"))
        .unwrap()
        .replace(
            r#"IF(?key = "status", LCASE(?value)"#,
            r#"IF(?key = "status", UCASE(?value)"#,
        );
    let sink = Request::new(
        Verb::Sink,
        Iri::parse(home.file("example").unwrap()).unwrap(),
    )
    .with_arg("content", ArgRef::Inline(edited.into_bytes()));
    futures::executor::block_on(kernel.issue(sink, &Capability::root())).expect("sink");

    let relifted = source(&kernel, request()).unwrap();
    assert!(relifted.contains("\"ACCEPTED\""), "{relifted}");
}

#[test]
fn a_missing_name_is_not_found_and_names_the_iri() {
    let scratch = Scratch::new("missing");
    scratch.install_example("example");
    let kernel = Kernel::new(Arc::new(mounted(&home(&scratch))));
    let expected = "urn:markdown:file:mappings/absent/mapping.ttl";

    match source(&kernel, named("absent")) {
        Err(Error::NotFound(msg)) => {
            assert!(msg.contains(expected), "{msg}");
            assert!(msg.contains("`absent`"), "{msg}");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }

    // Through the lift, the same typed error: no fallback to an unmapped lift.
    match source(
        &kernel,
        lift_request("# A\n", "urn:markdown:mapping:absent"),
    ) {
        Err(Error::NotFound(msg)) => assert!(msg.contains(expected), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

/// The miss hangs from the FILE's thread, not only the name's: installing the mapping
/// with a write through the kernel (which cuts the file's IRI) makes the name resolve,
/// with nothing cutting `urn:markdown:mapping:later` itself.
#[test]
fn installing_a_missing_mapping_through_the_kernel_un_misses_the_name() {
    let scratch = Scratch::new("later");
    scratch.install_example("example");
    let home = home(&scratch);
    let kernel = Kernel::new(Arc::new(mounted(&home)));
    assert!(matches!(
        source(&kernel, named("later")),
        Err(Error::NotFound(_))
    ));

    std::fs::create_dir_all(home.dir("later").unwrap()).unwrap();
    let sink = Request::new(Verb::Sink, Iri::parse(home.file("later").unwrap()).unwrap()).with_arg(
        "content",
        ArgRef::Inline(std::fs::read(mapping_path()).unwrap()),
    );
    futures::executor::block_on(kernel.issue(sink, &Capability::root())).expect("sink");

    let served = source(&kernel, named("later")).expect("installed");
    assert_eq!(
        served.as_bytes(),
        std::fs::read(mapping_path()).unwrap().as_slice()
    );
}

/// A host that binds no file mount serves no named mapping, and says why, rather than
/// the endpoint reaching for the disk itself.
#[test]
fn without_a_file_mount_a_name_is_not_found_and_says_what_is_missing() {
    let scratch = Scratch::new("unmounted");
    scratch.install_example("example");
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(home(&scratch))));
    match source(&kernel, named("example")) {
        Err(Error::NotFound(msg)) => {
            assert!(msg.contains("nothing serves"), "{msg}");
            assert!(msg.contains(ikigai_markdown::FILE_TEMPLATE), "{msg}");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

/// Reading a named mapping takes the authority reading its file takes, and the CALLER
/// must hold it: `ikigai-fs` checks the caller's capability against the file it opens.
#[test]
fn the_caller_needs_read_authority_over_the_mapping_file() {
    let scratch = Scratch::new("cap");
    scratch.install_example("example");
    let home = home(&scratch);
    let kernel = Kernel::new(Arc::new(mounted(&home)));

    let none = Capability::scoped(Vec::<String>::new());
    assert!(
        source_as(&kernel, named("example"), &none).is_err(),
        "no file grant: refused"
    );

    let mappings = home.root().unwrap().join("mappings");
    let read = Capability::scoped([format!("urn:cap:fs:read:{}", mappings.display())]);
    let served = source_as(&kernel, named("example"), &read).expect("a read grant serves it");
    assert_eq!(
        served.as_bytes(),
        std::fs::read(mapping_path()).unwrap().as_slice()
    );
}

#[test]
fn no_config_home_is_not_found_at_resolution() {
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space_with(Arc::new(
        MappingHome::new(None),
    ))));
    match source(&kernel, named("example")) {
        Err(Error::NotFound(msg)) => assert!(msg.contains("no config home"), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn a_name_cannot_leave_the_mappings_directory() {
    let scratch = Scratch::new("escape");
    let home = home(&scratch);
    for bad in ["", ".", "..", "a/b", "../x", "a b"] {
        assert!(
            matches!(home.file(bad), Err(Error::InvalidArgument { .. })),
            "`{bad}` must be refused"
        );
    }
    std::fs::create_dir_all(home.root().unwrap()).unwrap();
    let kernel = Kernel::new(Arc::new(mounted(&home)));
    for bad in ["..", "a b"] {
        let request = Request::new(
            Verb::Source,
            Iri::parse(format!("urn:markdown:mapping:{}", bad.replace(' ', "%20"))).unwrap(),
        );
        assert!(
            source(&kernel, request).is_err(),
            "`{bad}` must not be served"
        );
    }
}
