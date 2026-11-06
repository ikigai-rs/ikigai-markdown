//! Manual memory reproduction; run this test alone in a fresh process under /usr/bin/time -l.
use std::sync::Arc;

use ikigai_core::{
    ArgRef, Capability, Description, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation,
    Request, Verb,
};

#[test]
#[ignore = "manual snapshot peak-memory measurement"]
fn snapshot_memory() {
    let cached = std::env::var("SNAPSHOT_CACHE").as_deref() == Ok("1");
    let count: usize = std::env::var("SNAPSHOT_DOCS")
        .unwrap_or_else(|_| "400".into())
        .parse()
        .unwrap();
    let text = (0..1000)
        .map(|i| {
            format!(
                "## Heading {i}\n\nParagraph {i} with some text that the reduction discards.\n\n"
            )
        })
        .collect::<String>();
    let mut space = ikigai_markdown::space();
    let mut docs = Vec::new();
    for i in 0..count {
        let iri = format!("urn:memory:{i:04}.md");
        let bytes = text.as_bytes().to_vec();
        space = space.bind(
            Exact::new(iri.as_str()),
            FnEndpoint::new(format!("doc{i}"), move |_| {
                let repr = Representation::new(ReprType::new("text/markdown"), bytes.clone());
                Ok(if cached { repr.cacheable() } else { repr })
            })
            .with_description(
                Description::new(format!("doc{i}"))
                    .title("Measurement document")
                    .summary("Synthetic headings and paragraphs")
                    .verb(Verb::Source)
                    .output("text/markdown"),
            ),
        );
        docs.push(iri);
    }
    let kernel = Kernel::new(Arc::new(space));
    let request = Request::new(
        Verb::Source,
        Iri::parse(ikigai_markdown::SNAPSHOT_IRI).unwrap(),
    )
    .with_arg("docs", ArgRef::Inline(docs.join("\n").into_bytes()))
    .with_arg("root", ArgRef::Inline(b"urn:memory:".to_vec()));
    let repr = futures::executor::block_on(kernel.issue(request, &Capability::root())).unwrap();
    let lift = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(docs[count - 1].as_bytes().to_vec()))
        .with_arg(
            "path",
            ArgRef::Inline(format!("{:04}.md", count - 1).into_bytes()),
        );
    assert_eq!(kernel.is_cached(&lift, &Capability::root()), cached);
    eprintln!(
        "documents={count} source_bytes={} snapshot_bytes={} cached_lift={cached}",
        text.len() * count,
        repr.bytes.len()
    );
}
