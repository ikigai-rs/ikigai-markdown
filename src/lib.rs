//! `ikigai-markdown` — markdown as a graph.
//!
//! A standalone **ikigai module crate**: a host links it in and mounts [`space`].
//! It binds `urn:markdown:lift`, `urn:markdown:vocabulary` — the terms the lift
//! writes in — and, on hosts with a filesystem, `urn:markdown:mapping:{name}`: a
//! mapping found by name in the config home (see [`MappingHome`]).
//!
//! ```text
//! source urn:file:docs/notes/0007.md | urn:markdown:lift mapping=urn:file:notes.mapping.ttl
//! source urn:markdown:lift src=urn:file:notes/today.md as=application/trig
//! source urn:file:notes/0007.md | urn:markdown:lift mapping=urn:markdown:mapping:notes
//! ```
//!
//! ## Two stages, and only the first is code
//!
//! **Stage 1, the structural lift** ([`lift`]), is the only code, written once. It
//! turns markdown into a graph of what the document IS: frontmatter fields,
//! headings as sections with order and containment, list items with their text,
//! paragraphs, links, code blocks, tables. **It does not know what any document is
//! about** — no word of any project's vocabulary appears in it.
//!
//! **Stage 2, the mapping** ([`Mapping`]), is a resource. It carries a **lift
//! profile** — data telling stage 1 which frontmatter keys are delimited lists and
//! which regexes are citations — and one or more **SPARQL CONSTRUCTs** that read
//! the structural graph and write the domain model. Three teams with three document
//! conventions are three mapping files and zero lines of code.
//!
//! The profile exists because of one wall: SPARQL 1.1 cannot split a string into
//! rows, so a reference to something that does NOT exist could never be seen by a
//! query — a regex join only ever tests candidates that exist. Stage 1 therefore
//! emits each split piece and each pattern match as a node, driven entirely by the
//! profile, and dangling references become queryable data.
//!
//! ## Caching: two golden threads
//!
//! A lift is a pure function of **(document bytes, mapping bytes)**. `src` and
//! `mapping` are each either a resource IRI, resolved through the kernel, or the
//! thing itself by value. By reference, the kernel folds the resource's golden
//! thread into the result, so a lift over a watched file and a watched mapping is
//! `.cacheable()` and recomputes when EITHER is cut — editing the mapping re-lifts
//! every document under it, with no rebuild. Get this wrong and every derived graph
//! is silently stale the moment someone tunes the mapping.
//!
//! ## Capabilities: none of its own
//!
//! The lift reads nothing but its arguments and what it resolves through the
//! kernel, and every sub-resolution runs under the caller's capability — so the
//! caller needs whatever reading `src` and `mapping` needs, enforced where they are
//! served, and nothing more.

#![forbid(unsafe_code)]

mod lift;
mod map;
mod mapping;
#[cfg(not(target_family = "wasm"))]
mod named;
mod sparql;
mod vocab;

pub use lift::{lift, DocRef};
pub use map::{apply, serialize};
pub use mapping::{Citation, Mapping, Profile, Split};
#[cfg(not(target_family = "wasm"))]
pub use named::{mapping_endpoint, MappingHome, MAPPING_ID, MAPPING_TEMPLATE};
pub use vocab::MD;

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint, Invocation, Iri,
    ReprType, Representation, Result, Verb,
};
use oxigraph::model::NamedNode;

/// The lift's IRI.
pub const LIFT_IRI: &str = "urn:markdown:lift";
/// The lift's description id.
pub const LIFT_ID: &str = "markdown-lift";
/// The default output: one named graph per document, line-oriented and sortable.
pub const NQUADS: &str = "application/n-quads";
/// The other output, for reading.
pub const TRIG: &str = "application/trig";
/// The IRI prefix of a document lifted by value under a `path`.
pub const DOC_PREFIX: &str = "urn:markdown:doc:";

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// The vocabulary's IRI.
pub const VOCABULARY_IRI: &str = "urn:markdown:vocabulary";
/// The vocabulary's description id.
pub const VOCABULARY_ID: &str = "markdown-vocabulary";
/// The structural vocabulary, as Turtle: every `md:` term the lift or a mapping uses.
pub const VOCABULARY: &str = include_str!("vocabulary.ttl");

/// The module's space over **this machine's** config home: `urn:markdown:lift` and
/// `urn:markdown:vocabulary` everywhere, plus `urn:markdown:mapping:{name}` on hosts
/// that have a filesystem. Mount this in a host kernel's root.
///
/// On wasm the named-mapping endpoint is absent rather than present-and-failing:
/// an action in the manifold that cannot succeed is worse than one not offered.
pub fn space() -> EndpointSpace {
    #[cfg(target_family = "wasm")]
    {
        base_space()
    }
    #[cfg(not(target_family = "wasm"))]
    {
        space_with(std::sync::Arc::new(MappingHome::ambient()))
    }
}

/// The module's space with named mappings looked up under a config home the caller
/// states — what a host serving another home mounts, and what a test mounts so the
/// mappings it reads are ones it wrote.
#[cfg(not(target_family = "wasm"))]
pub fn space_with(home: std::sync::Arc<MappingHome>) -> EndpointSpace {
    base_space().bind(
        ikigai_core::UriTemplate::parse(MAPPING_TEMPLATE)
            .expect("MAPPING_TEMPLATE is a valid template"),
        mapping_endpoint(home),
    )
}

fn base_space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new(LIFT_IRI), LiftEndpoint)
        .bind(Exact::new(VOCABULARY_IRI), vocabulary_endpoint())
}

/// The vocabulary the lift's graph is written in, served so the terms are defined
/// where they are used rather than invented.
fn vocabulary_endpoint() -> FnEndpoint {
    FnEndpoint::new(VOCABULARY_ID, |_: &Invocation<'_>| {
        Ok(
            Representation::new(ReprType::new("text/turtle"), VOCABULARY.as_bytes().to_vec())
                .cacheable(),
        )
    })
    .with_description(
        Description::new(VOCABULARY_ID)
            .title("Markdown structural vocabulary")
            .summary(
                "The md: vocabulary (https://ikigai-rs.dev/ns/md#): the classes and properties \
                 of a lifted markdown document and of a mapping resource.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .output("text/turtle"),
    )
}

struct LiftEndpoint;

#[async_trait]
impl Endpoint for LiftEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        // The document: `src` as a reference or by value, else `content` by value.
        let (markdown, source) = match inv.inline_str("src") {
            Ok(src) if is_reference(src) => {
                let iri = parse_iri("src", src)?;
                (text(inv.source(&iri).await?, "src")?, Some(src.to_string()))
            }
            Ok(src) => (src.to_string(), None),
            Err(_) => match inv.inline_str("content") {
                Ok(content) => (content.to_string(), None),
                Err(_) => {
                    return Err(Error::InvalidArgument {
                        name: "src".to_string(),
                        detail: "urn:markdown:lift needs `src` (a resource IRI or the markdown \
                                 itself) or a piped document"
                            .to_string(),
                    })
                }
            },
        };

        // The mapping: optional; a reference or Turtle by value.
        let mapping = match inv.inline_str("mapping") {
            Err(_) => Mapping::default(),
            Ok(m) => {
                let (turtle, base) = if is_reference(m) {
                    let iri = parse_iri("mapping", m)?;
                    (text(inv.source(&iri).await?, "mapping")?, m.to_string())
                } else {
                    (m.to_string(), "urn:markdown:mapping:inline".to_string())
                };
                Mapping::parse_checked(&turtle, &base)?
            }
        };

        let trig = match inv.inline_str("as").map(str::trim) {
            Err(_) | Ok(NQUADS) => false,
            Ok(TRIG) => true,
            Ok(other) => {
                return Err(Error::InvalidArgument {
                    name: "as".to_string(),
                    detail: format!("expected {NQUADS} or {TRIG}, got {other:?}"),
                })
            }
        };

        let doc = doc_ref(inv.inline_str("path").ok(), source.as_deref())?;
        let mut quads = lift(&markdown, &doc, &mapping.profile);
        let domain = map::apply_checked(&quads, &doc.iri, &mapping.constructs)?;
        quads.extend(domain);
        let bytes = serialize(&quads, trig).map_err(Error::Endpoint)?;
        let media = if trig { TRIG } else { NQUADS };
        // Cacheable: a function of the arguments, plus whatever `src` and `mapping`
        // resolved to — the kernel folds in their threads and expiry.
        Ok(Representation::new(ReprType::new(media), bytes).cacheable())
    }

    fn name(&self) -> &str {
        LIFT_ID
    }

    fn describe(&self) -> Description {
        Description::new(LIFT_ID)
            .title("Markdown lift")
            .summary(
                "Lift markdown into RDF: a generic structural graph (frontmatter, sections, \
                 lists, links, code), plus the domain triples a mapping resource's lift \
                 profile and SPARQL CONSTRUCTs derive from it. One named graph per document.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("src")
                    .summary(
                        "the document: a resolvable resource IRI (its golden thread is kept), \
                         or the markdown itself — pipe it in. A value that is one token shaped \
                         like an absolute IRI is read as a reference; pass `content=` to force \
                         by-value.",
                    )
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("content")
                    .summary("the markdown by value; `src` takes precedence when both are given")
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(
                ArgSpec::new("mapping")
                    .summary(
                        "the mapping resource: an IRI (its golden thread is kept) or Turtle by \
                         value — one md:Mapping with md:split, md:citation and md:construct. \
                         urn:markdown:mapping:{name} names one installed in the config home. \
                         Omitted, only the structural graph is produced.",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(
                ArgSpec::new("path")
                    .summary(
                        "the document's logical path (md:path); names the graph \
                         urn:markdown:doc:{path}. Omitted, a by-reference document's graph is \
                         its own IRI.",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(
                ArgSpec::new("as")
                    .summary("output media type")
                    .class(XSD_STRING)
                    .one_of([NQUADS, TRIG])
                    .default_value(NQUADS)
                    .optional(),
            )
            .output(NQUADS)
            .output(TRIG)
    }
}

/// Whether an argument value is a reference rather than the thing itself: a single
/// token with a URI scheme. Markdown or Turtle that happens to be exactly one such
/// token is misread; `content=` (for markdown) is the explicit by-value form.
fn is_reference(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !value.chars().any(char::is_whitespace)
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn parse_iri(name: &str, value: &str) -> Result<Iri> {
    Iri::parse(value).map_err(|e| Error::InvalidArgument {
        name: name.to_string(),
        detail: format!("`{value}` is not a valid IRI: {e}"),
    })
}

fn text(repr: Representation, role: &str) -> Result<String> {
    String::from_utf8(repr.bytes).map_err(|e| Error::InvalidArgument {
        name: role.to_string(),
        detail: format!("{role} is not valid UTF-8: {e}"),
    })
}

/// Name the document: `urn:markdown:doc:{path}` when a path is given, else the IRI
/// it was resolved from, else `urn:markdown:doc:anonymous`.
fn doc_ref(path: Option<&str>, source: Option<&str>) -> Result<DocRef> {
    let invalid = |detail: String| Error::InvalidArgument {
        name: "path".to_string(),
        detail,
    };
    let source_node = source
        .map(|s| NamedNode::new(s).map_err(|e| invalid(format!("`{s}`: {e}"))))
        .transpose()?;
    let (iri, path) = match (path.map(str::trim).filter(|p| !p.is_empty()), source) {
        (Some(p), _) => (format!("{DOC_PREFIX}{}", escape_path(p)), p.to_string()),
        (None, Some(s)) => (s.to_string(), s.to_string()),
        (None, None) => (format!("{DOC_PREFIX}anonymous"), String::new()),
    };
    let iri = NamedNode::new(&iri).map_err(|e| invalid(format!("`{iri}`: {e}")))?;
    Ok(DocRef {
        iri,
        path,
        source: source_node,
    })
}

/// Percent-encode everything in a path that is not safe in an IRI path segment
/// (keeping `/`).
fn escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_one_token_with_a_scheme() {
        assert!(is_reference("urn:file:docs/a.md"));
        assert!(is_reference("https://example.org/a.md"));
        assert!(!is_reference("# Title\n\nurn:file:x"));
        assert!(!is_reference("plain words"));
        assert!(!is_reference("no-scheme"));
        assert!(!is_reference("2026:x y"));
        assert!(!is_reference("@prefix md: <x> ."));
    }

    #[test]
    fn a_path_names_the_graph_and_is_escaped() {
        let d = doc_ref(Some("docs/a b.md"), Some("urn:file:x")).unwrap();
        assert_eq!(d.iri.as_str(), "urn:markdown:doc:docs/a%20b.md");
        assert_eq!(d.path, "docs/a b.md");
        assert_eq!(d.source.unwrap().as_str(), "urn:file:x");
        let d = doc_ref(None, Some("urn:file:x.md")).unwrap();
        assert_eq!(d.iri.as_str(), "urn:file:x.md");
        let d = doc_ref(None, None).unwrap();
        assert_eq!(d.iri.as_str(), "urn:markdown:doc:anonymous");
    }
}
