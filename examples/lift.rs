//! Lift one markdown file through the kernel and print the graph.
//!
//! ```text
//! cargo run --example lift -- <file.md> [mapping.ttl] [path]
//! ```
//!
//! The file and the mapping are bound as kernel resources and passed BY REFERENCE,
//! exactly as a host serving them from `ikigai-fs` would, so this is the real call.

use std::sync::Arc;

use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};

fn file(id: &str, media: &'static str, bytes: Vec<u8>) -> FnEndpoint {
    FnEndpoint::new(id.to_string(), move |_| {
        Ok(Representation::new(ReprType::new(media), bytes.clone()))
    })
    .with_description(
        Description::new(id)
            .summary("a file bound for the example")
            .verb(Verb::Source),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(md) = args.first() else {
        eprintln!("usage: lift <file.md> [mapping.ttl] [path]");
        std::process::exit(2);
    };
    let mut space: EndpointSpace = ikigai_markdown::space().bind(
        Exact::new("urn:example:doc"),
        file(
            "doc",
            "text/markdown",
            std::fs::read(md).expect("read markdown"),
        ),
    );
    let mut request = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(b"urn:example:doc".to_vec()))
        .with_arg("as", ArgRef::Inline(ikigai_markdown::TRIG.into()));
    if let Some(mapping) = args.get(1) {
        space = space.bind(
            Exact::new("urn:example:mapping"),
            file(
                "mapping",
                "text/turtle",
                std::fs::read(mapping).expect("read mapping"),
            ),
        );
        request = request.with_arg("mapping", ArgRef::Inline(b"urn:example:mapping".to_vec()));
    }
    if let Some(path) = args.get(2) {
        request = request.with_arg("path", ArgRef::Inline(path.as_bytes().to_vec()));
    }
    let kernel = Kernel::new(Arc::new(space));
    match futures::executor::block_on(kernel.issue(request, &Capability::root())) {
        Ok(repr) => print!("{}", String::from_utf8_lossy(&repr.bytes)),
        Err(e) => {
            eprintln!("lift failed: {e}");
            std::process::exit(1);
        }
    }
}
