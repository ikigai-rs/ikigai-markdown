//! The mapping resource: a lift profile plus the CONSTRUCTs that interpret the
//! structural graph.
//!
//! A mapping is Turtle, so it is itself a graph — queryable, diffable, and
//! validatable like anything else in the system. Exactly one subject is typed
//! `md:Mapping`; it carries three kinds of statement:
//!
//! ```turtle
//! @prefix md: <https://ikigai-rs.dev/ns/md#> .
//! <#m> a md:Mapping ;
//!     md:split    [ md:key "tags" ; md:delimiter "," ] ;
//!     md:citation [ md:name "issue" ; md:pattern "#(?P<v>[0-9]+)" ] ;
//!     md:construct """PREFIX md: <https://ikigai-rs.dev/ns/md#>
//!                     CONSTRUCT { ?d <urn:ex:title> ?t }
//!                     WHERE { ?h a md:Heading ; md:document ?d ; md:level 1 ; md:text ?t }""" .
//! ```
//!
//! - **`md:split`** — a frontmatter key whose value is a delimited list. Stage 1
//!   splits it on the `md:delimiter` regex (default `,`) and emits each trimmed,
//!   non-empty piece as an `md:Token`. This exists because SPARQL 1.1 cannot turn
//!   one string into several rows, so a reference to an entity that does not exist
//!   could never be seen without it.
//! - **`md:citation`** — a named regex scanned over every line of the source. Each
//!   match becomes an `md:Match` carrying the whole match (`md:text`), its
//!   `md:value` — the named group `v`, else group 1, else the whole match — its
//!   line, and the innermost block and nearest section it falls in. The pattern is a
//!   Rust `regex`; the constructs are SPARQL, whose REGEX is XPath-flavoured.
//! - **`md:construct`** — one or more SPARQL CONSTRUCT queries, run independently
//!   over the document's structural graph. Their union is the document's domain
//!   graph. They must not emit blank nodes: build IRIs with `IRI(CONCAT(…))`.
//!
//! ⚠ TRUST. A mapping decides how someone's documents are INTERPRETED. It cannot
//! execute anything — a regex and a SPARQL query over one in-memory document — so
//! reading one from a cloned repository risks misinterpretation, not compromise.
//! Treat a stranger's mapping as a claim about meaning, not as configuration.

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{NamedOrBlankNode, Term, Triple};
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use regex::Regex;

use crate::vocab::{md, RDF_TYPE};

/// A frontmatter key to split into tokens.
#[derive(Debug, Clone)]
pub struct Split {
    /// The frontmatter key, as written (nested keys are dot-joined).
    pub key: String,
    /// The delimiter, a regex.
    pub delimiter: Regex,
}

/// A named pattern whose matches become `md:Match` nodes.
#[derive(Debug, Clone)]
pub struct Citation {
    /// The name a mapping's CONSTRUCT filters matches by (`md:matchedBy`).
    pub name: String,
    /// The regex. The named group `v`, else group 1, is the match's `md:value`.
    pub pattern: Regex,
}

/// The data-driven half of stage 1. Empty by default: no mapping still lifts.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// Frontmatter keys to split.
    pub splits: Vec<Split>,
    /// Patterns to scan for.
    pub citations: Vec<Citation>,
}

/// A parsed mapping resource.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    /// The lift profile stage 1 applies.
    pub profile: Profile,
    /// The CONSTRUCT queries stage 2 runs, as written.
    pub constructs: Vec<String>,
}

impl Mapping {
    /// Parse a mapping from Turtle, resolving relative IRIs against `base`.
    ///
    /// Refuses — naming the problem — a mapping with no or several `md:Mapping`
    /// subjects, a split or citation missing its required field, a regex that does
    /// not compile, and a `md:construct` that is not a CONSTRUCT query.
    ///
    /// ```
    /// let ttl = r##"@prefix md: <https://ikigai-rs.dev/ns/md#> .
    /// <#m> a md:Mapping ; md:split [ md:key "tags" ] ;
    ///     md:citation [ md:name "issue" ; md:pattern "#(?P<v>[0-9]+)" ] ."##;
    /// let m = ikigai_markdown::Mapping::parse(ttl, "urn:ex:mapping").unwrap();
    /// assert_eq!(m.profile.splits[0].key, "tags");
    /// assert_eq!(m.profile.splits[0].delimiter.as_str(), ",");
    /// assert_eq!(m.profile.citations[0].name, "issue");
    /// assert!(m.constructs.is_empty());
    /// ```
    pub fn parse(turtle: &str, base: &str) -> Result<Mapping, String> {
        let parser = RdfParser::from_format(RdfFormat::Turtle)
            .with_base_iri(base)
            .map_err(|e| format!("mapping base IRI `{base}`: {e}"))?;
        let mut triples: Vec<Triple> = Vec::new();
        for quad in parser.for_slice(turtle.as_bytes()) {
            let quad = quad.map_err(|e| format!("mapping is not valid Turtle: {e}"))?;
            triples.push(Triple::new(quad.subject, quad.predicate, quad.object));
        }

        let mapping_class = Term::from(md("Mapping"));
        let subjects: Vec<&NamedOrBlankNode> = triples
            .iter()
            .filter(|t| t.predicate.as_str() == RDF_TYPE && t.object == mapping_class)
            .map(|t| &t.subject)
            .collect();
        let subject = match subjects.as_slice() {
            [one] => (*one).clone(),
            [] => return Err("mapping declares no subject typed md:Mapping".to_string()),
            many => {
                return Err(format!(
                    "mapping declares {} subjects typed md:Mapping; exactly one is allowed",
                    many.len()
                ))
            }
        };

        let objects = |s: &NamedOrBlankNode, p: &str| -> Vec<Term> {
            let p = md(p);
            triples
                .iter()
                .filter(|t| &t.subject == s && t.predicate == p)
                .map(|t| t.object.clone())
                .collect()
        };
        let literal = |s: &NamedOrBlankNode, p: &str| -> Result<Option<String>, String> {
            match objects(s, p).as_slice() {
                [] => Ok(None),
                [Term::Literal(l)] => Ok(Some(l.value().to_string())),
                [_] => Err(format!("md:{p} must be a literal")),
                _ => Err(format!("md:{p} is given more than once")),
            }
        };
        let node = |t: Term, p: &str| -> Result<NamedOrBlankNode, String> {
            match t {
                Term::NamedNode(n) => Ok(n.into()),
                Term::BlankNode(b) => Ok(b.into()),
                _ => Err(format!("md:{p} must point at a node, not a literal")),
            }
        };

        let mut profile = Profile::default();
        for t in objects(&subject, "split") {
            let s = node(t, "split")?;
            let key = literal(&s, "key")?.ok_or("an md:split has no md:key")?;
            let delimiter = literal(&s, "delimiter")?.unwrap_or_else(|| ",".to_string());
            let delimiter = Regex::new(&delimiter)
                .map_err(|e| format!("md:split `{key}`: delimiter is not a regex: {e}"))?;
            profile.splits.push(Split { key, delimiter });
        }
        for t in objects(&subject, "citation") {
            let s = node(t, "citation")?;
            let name = literal(&s, "name")?.ok_or("an md:citation has no md:name")?;
            let pattern = literal(&s, "pattern")?
                .ok_or_else(|| format!("md:citation `{name}` has no md:pattern"))?;
            let pattern = Regex::new(&pattern)
                .map_err(|e| format!("md:citation `{name}`: pattern is not a regex: {e}"))?;
            profile.citations.push(Citation { name, pattern });
        }
        // Parse order is the file's order; sort so the lift's node numbering does
        // not depend on how the author arranged the Turtle.
        profile.splits.sort_by(|a, b| a.key.cmp(&b.key));
        profile.citations.sort_by(|a, b| a.name.cmp(&b.name));

        let mut constructs = Vec::new();
        for t in objects(&subject, "construct") {
            let Term::Literal(l) = t else {
                return Err("md:construct must be a literal holding a query".to_string());
            };
            // Syntax, then form: evaluated over an empty store, a CONSTRUCT answers
            // with a graph and anything else does not.
            let empty = Store::new().map_err(|e| format!("in-memory store: {e}"))?;
            let results = SparqlEvaluator::new()
                .parse_query(l.value())
                .map_err(|e| format!("md:construct is not valid SPARQL: {e}"))?
                .on_store(&empty)
                .execute()
                .map_err(|e| format!("md:construct does not evaluate: {e}"))?;
            if !matches!(results, QueryResults::Graph(_)) {
                return Err("md:construct must be a CONSTRUCT query".to_string());
            }
            constructs.push(l.value().to_string());
        }
        constructs.sort();

        Ok(Mapping {
            profile,
            constructs,
        })
    }
}
