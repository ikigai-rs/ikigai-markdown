//! The structural vocabulary: what a markdown document IS, with no opinion about
//! what it is ABOUT.
//!
//! Every term here names a CommonMark/GFM construct (a heading, a list item, a
//! frontmatter field) or a lift-profile operation (a split, a citation pattern).
//! None names anything a particular document process means by its documents —
//! that vocabulary belongs to a mapping resource, never to this file.
//!
//! ⚠ The namespace `https://ikigai-rs.dev/ns/md#` is NOT yet deployed at /ns. It is
//! owned by this module until the hub decides whether it joins the shared
//! vocabulary (a core change: vocabulary.ttl, a vocab publish, a manual /ns deploy).

use oxigraph::model::NamedNode;

/// The structural namespace.
pub const MD: &str = "https://ikigai-rs.dev/ns/md#";

/// `xsd:integer`.
pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
/// `xsd:boolean`.
pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
/// `rdf:type`.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// A term in the structural namespace.
pub fn md(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{MD}{local}"))
}
