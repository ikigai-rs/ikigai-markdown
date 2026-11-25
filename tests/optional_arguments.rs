//! Optional arguments distinguish absence from unreadable values at every endpoint.
use ikigai_core::{
    ArgRef, Capability, Error, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation, Request,
    Verb,
};
use std::sync::Arc;

fn kernel() -> Kernel {
    let document = FnEndpoint::new("document", |_: &ikigai_core::Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new("text/markdown"),
            b"# Hello".to_vec(),
        ))
    });
    Kernel::new(Arc::new(
        ikigai_markdown::space().bind(Exact::new("urn:test:doc"), document),
    ))
}

fn request(target: &str) -> Request {
    let mut req = Request::new(Verb::Source, Iri::parse(target).unwrap());
    if target == ikigai_markdown::LIFT_IRI {
        req = req.with_arg("src", ArgRef::Inline(b"# Hello".to_vec()));
    } else {
        if target == ikigai_markdown::SNAPSHOT_IRI || target == ikigai_markdown::LINT_CORPUS_IRI {
            req = req.with_arg("docs", ArgRef::Inline(b"urn:test:doc".to_vec()));
        }
        if target != ikigai_markdown::SNAPSHOT_IRI {
            req = req.with_arg(
                "mapping",
                ArgRef::Inline(include_bytes!("../mappings/example/mapping.ttl").to_vec()),
            );
        }
    }
    req
}

#[test]
fn optional_arguments_accept_absent_and_valid_values_but_refuse_invalid_utf8() {
    let cases = [
        (
            ikigai_markdown::LIFT_IRI,
            "mapping",
            include_str!("../mappings/example/mapping.ttl"),
        ),
        (ikigai_markdown::LIFT_IRI, "as", "application/trig"),
        (ikigai_markdown::LIFT_IRI, "path", "hello.md"),
        (ikigai_markdown::SNAPSHOT_IRI, "root", "urn:test:"),
        (
            ikigai_markdown::SNAPSHOT_IRI,
            "mapping",
            include_str!("../mappings/example/mapping.ttl"),
        ),
        (ikigai_markdown::LINT_MAPPING_IRI, "as", "text/turtle"),
        (ikigai_markdown::LINT_EXPECTS_IRI, "as", "text/turtle"),
        (ikigai_markdown::LINT_CORPUS_IRI, "as", "text/turtle"),
        (ikigai_markdown::LINT_CORPUS_IRI, "root", "urn:test:"),
        (
            ikigai_markdown::LINT_CORPUS_IRI,
            "ignore",
            "urn:test:predicate",
        ),
        (ikigai_markdown::LINT_CORPUS_IRI, "baseline", ""),
    ];
    for (target, name, valid) in cases {
        let k = kernel();
        for value in [None, Some(valid)] {
            let mut req = request(target);
            if let Some(value) = value {
                req = req.with_arg(name, ArgRef::Inline(value.as_bytes().to_vec()));
            }
            let result = futures::executor::block_on(k.issue(req, &Capability::root()));
            assert!(result.is_ok(), "{target} {name} {value:?}: {result:?}");
        }
        let req = request(target).with_arg(name, ArgRef::Inline(vec![0xff]));
        let result = futures::executor::block_on(k.issue(req, &Capability::root()));
        match result {
            Err(Error::InvalidArgument {
                name: actual,
                detail,
            }) => {
                assert_eq!(actual, name, "{target}");
                assert!(detail.contains("UTF-8"), "{target} {name}: {detail}");
            }
            other => panic!("{target} {name}: expected invalid UTF-8 refusal, got {other:?}"),
        }
    }
}

#[test]
fn invalid_src_does_not_fall_back_to_content() {
    let req = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(vec![0xff]))
        .with_arg("content", ArgRef::Inline(b"# fallback".to_vec()));
    assert!(
        matches!(futures::executor::block_on(kernel().issue(req, &Capability::root())), Err(Error::InvalidArgument { name, .. }) if name == "src")
    );
}
