//! What a mapping's constructs DEPEND ON, read from their algebra without running them.
//!
//! A construct reads the structural graph through a handful of `md:` predicates, and the
//! words a document author can break are the literals it compares those values with:
//! `md:key "related"`, `?h md:text "Record"`, `FILTER(?key = "date")`, a `VALUES` row, a
//! `REGEX(?path, …)`. This module walks each construct's algebra (spargebra's, the same
//! parse the lift runs) and reports every such literal against the structural value it
//! is compared with, plus what the construct WRITES (its template's predicates and
//! classes). Nothing here evaluates a query.
//!
//! The walk follows values through `BIND`: a variable bound from `LCASE(?k)` compares
//! case-insensitively with whatever `?k` came from, and one bound from `REPLACE(?raw, …)`
//! is reported as a value COMPUTED from `md:raw`, not as `md:raw` itself. It is a reading,
//! not a proof: a construct that builds a comparison at run time (a literal assembled with
//! `CONCAT`, a predicate chosen by `IRI(…)`) is reported as computed, never guessed.

use std::collections::{BTreeMap, BTreeSet};

use spargebra::algebra::{Expression, Function, GraphPattern};
use spargebra::term::{GroundTerm, Literal, NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;

use crate::vocab::{MD, RDF_TYPE};

/// What a literal is compared with.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExpectKind {
    /// A document path (`md:path`).
    Path,
    /// A frontmatter key (`md:key`).
    FrontmatterKey,
    /// A heading's text (`md:text` of an `md:Heading`).
    Heading,
    /// A heading's level (`md:level` of an `md:Heading`).
    HeadingLevel,
    /// A citation pattern's name (`md:matchedBy`).
    CitationName,
    /// Any other structural value, or one computed from it; `on` says which.
    Value,
}

impl ExpectKind {
    /// The kind's name on the wire (`md:kind`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ExpectKind::Path => "path",
            ExpectKind::FrontmatterKey => "frontmatter-key",
            ExpectKind::Heading => "heading",
            ExpectKind::HeadingLevel => "heading-level",
            ExpectKind::CitationName => "citation-name",
            ExpectKind::Value => "value",
        }
    }
}

/// How the literal is compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum How {
    /// Equal to it.
    Equals,
    /// Equal to it ignoring case (the value went through `LCASE`/`UCASE`).
    EqualsIgnoringCase,
    /// Matching it as a regular expression (`REGEX`).
    Matches,
    /// Containing it.
    Contains,
    /// Starting with it.
    StartsWith,
    /// Ending with it.
    EndsWith,
}

impl How {
    /// The comparison's name on the wire (`md:how`).
    pub fn as_str(&self) -> &'static str {
        match self {
            How::Equals => "equals",
            How::EqualsIgnoringCase => "equals-ignoring-case",
            How::Matches => "matches",
            How::Contains => "contains",
            How::StartsWith => "starts-with",
            How::EndsWith => "ends-with",
        }
    }
}

/// One literal a construct depends on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Expect {
    /// What it is compared with.
    pub kind: ExpectKind,
    /// For [`ExpectKind::Value`], the structural value: `md:raw of md:ListItem`, or
    /// `computed from md:path`. Empty for the other kinds.
    pub on: String,
    /// The literal's lexical form.
    pub value: String,
    /// How it is compared.
    pub how: How,
}

/// What one construct writes: its template's constant predicates and classes.
#[derive(Debug, Clone, Default)]
pub struct Produces {
    /// Predicate IRIs the template writes, including those a variable takes from a
    /// `VALUES` row or a `BIND` of a constant.
    pub predicates: BTreeSet<String>,
    /// Classes the template types its subjects with, by the same rule.
    pub classes: BTreeSet<String>,
    /// Some predicate is computed at run time and cannot be read here.
    pub computed_predicate: bool,
    /// Some class is computed at run time and cannot be read here.
    pub computed_class: bool,
    /// The template mints a blank node, which the lift refuses.
    pub blank_node: bool,
}

/// One construct, read.
#[derive(Debug, Clone, Default)]
pub struct ConstructFacts {
    /// Its 1-based position in the mapping file.
    pub index: usize,
    /// Why the lift would refuse it, if it would.
    pub problem: Option<String>,
    /// The literals it depends on.
    pub expects: BTreeSet<Expect>,
    /// What it writes.
    pub produces: Produces,
    /// It reads `md:matchedBy` into a variable, so it may read citations of any name.
    pub reads_any_citation: bool,
}

/// Read construct `index` (1-based) of a mapping. Its text is caller text: the bounds the
/// lift applies are applied here first, and the parse and the walk run on a thread sized
/// for the text, as the lift's do.
pub(crate) fn construct(text: &str, index: usize) -> ConstructFacts {
    let what = format!("construct {index}");
    let read = ikigai_store::limits::on_sparql_stack(text, || {
        let query = crate::sparql::parse_static(text, &what)?;
        let Query::Construct {
            template, pattern, ..
        } = &query
        else {
            return Err(crate::sparql::refuse(format!(
                "{what} is not a CONSTRUCT query"
            )));
        };
        Ok(read_construct(template, pattern))
    });
    match read {
        Ok(mut facts) => {
            facts.index = index;
            facts
        }
        Err(e) => ConstructFacts {
            index,
            problem: Some(crate::sparql::detail(e)),
            ..ConstructFacts::default()
        },
    }
}

/// Where a variable's value came from: a structural predicate, the role of the node it was
/// read from, and what was done to it on the way.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Origin {
    /// The `md:` local name of the predicate it was read through.
    predicate: String,
    /// The node's class or classes (`Heading`, `ListItem`), `/`-joined; empty if unknown.
    role: String,
    /// Passed through a function other than a case fold.
    computed: bool,
    /// Passed through `LCASE`/`UCASE`.
    folded: bool,
}

/// A `VALUES` table's rows.
type Rows<'q> = &'q [Vec<Option<GroundTerm>>];

#[derive(Default)]
struct Walk<'q> {
    triples: Vec<&'q TriplePattern>,
    extends: Vec<(&'q str, &'q Expression)>,
    expressions: Vec<&'q Expression>,
    values: Vec<(Vec<&'q str>, Rows<'q>)>,
}

impl<'q> Walk<'q> {
    fn pattern(&mut self, p: &'q GraphPattern) {
        match p {
            GraphPattern::Bgp { patterns } => self.triples.extend(patterns.iter()),
            GraphPattern::Join { left, right }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => {
                self.pattern(left);
                self.pattern(right);
            }
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => {
                self.pattern(left);
                self.pattern(right);
                if let Some(e) = expression {
                    self.expression(e);
                }
            }
            GraphPattern::Filter { expr, inner } => {
                self.expression(expr);
                self.pattern(inner);
            }
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => {
                self.pattern(inner);
                self.extends.push((variable.as_str(), expression));
                self.expression(expression);
            }
            GraphPattern::Values {
                variables,
                bindings,
            } => self.values.push((
                variables.iter().map(|v| v.as_str()).collect(),
                bindings.as_slice(),
            )),
            GraphPattern::Graph { inner, .. }
            | GraphPattern::Service { inner, .. }
            | GraphPattern::OrderBy { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::Group { inner, .. } => self.pattern(inner),
            // A property path names no literal this reading tracks, and a lateral join (a
            // feature-gated variant) is not read: what it compares is not reported.
            #[allow(unreachable_patterns)] // which variants exist depends on spargebra's features
            _ => {}
        }
    }

    /// Record an expression for the comparison pass, and walk any `EXISTS` inside it.
    fn expression(&mut self, e: &'q Expression) {
        self.expressions.push(e);
        let mut stack = vec![e];
        while let Some(e) = stack.pop() {
            if let Expression::Exists(p) = e {
                self.pattern(p);
            }
            stack.extend(children(e));
        }
    }
}

/// An expression's direct sub-expressions.
fn children(e: &Expression) -> Vec<&Expression> {
    match e {
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => vec![a, b],
        Expression::In(a, list) => std::iter::once(&**a).chain(list.iter()).collect(),
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => vec![a],
        Expression::If(a, b, c) => vec![a, b, c],
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => list.iter().collect(),
        _ => Vec::new(),
    }
}

/// Every variable an expression reads, outside any `EXISTS`.
fn variables(e: &Expression) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        match e {
            Expression::Variable(v) => {
                out.insert(v.as_str());
            }
            Expression::Bound(v) => {
                out.insert(v.as_str());
            }
            _ => stack.extend(children(e)),
        }
    }
    out
}

/// What an expression does to the values it reads: (folds case, computes otherwise).
/// `IF`'s condition and `COALESCE`'s choice pass a value through unchanged.
fn transform(e: &Expression) -> (bool, bool) {
    let (mut folded, mut computed) = (false, false);
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        match e {
            Expression::FunctionCall(Function::LCase | Function::UCase | Function::Str, args) => {
                folded |= !matches!(e, Expression::FunctionCall(Function::Str, _));
                stack.extend(args.iter());
            }
            Expression::FunctionCall(_, args) => {
                computed = true;
                stack.extend(args.iter());
            }
            Expression::Add(..)
            | Expression::Subtract(..)
            | Expression::Multiply(..)
            | Expression::Divide(..)
            | Expression::UnaryPlus(..)
            | Expression::UnaryMinus(..) => {
                computed = true;
                stack.extend(children(e));
            }
            _ => stack.extend(children(e)),
        }
    }
    (folded, computed)
}

/// The IRI constants an expression can evaluate to (`IF(…, ex:A, ex:B)`, a bare `ex:A`).
fn constants(e: &Expression, out: &mut BTreeSet<String>) {
    match e {
        Expression::NamedNode(n) => {
            out.insert(n.as_str().to_string());
        }
        Expression::If(_, b, c) => {
            constants(b, out);
            constants(c, out);
        }
        Expression::Coalesce(list) => {
            for e in list {
                constants(e, out);
            }
        }
        _ => {}
    }
}

fn key(t: &TermPattern) -> Option<String> {
    match t {
        TermPattern::Variable(v) => Some(format!("?{}", v.as_str())),
        TermPattern::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        _ => None,
    }
}

fn md_local(p: &NamedNodePattern) -> Option<&str> {
    match p {
        NamedNodePattern::NamedNode(n) => n.as_str().strip_prefix(MD),
        NamedNodePattern::Variable(_) => None,
    }
}

fn read_construct(template: &[TriplePattern], pattern: &GraphPattern) -> ConstructFacts {
    let mut walk = Walk::default();
    walk.pattern(pattern);

    // Roles: what each node of the pattern is, from its type and how it is reached.
    let mut roles: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for t in &walk.triples {
        let (Some(s), local) = (key(&t.subject), md_local(&t.predicate)) else {
            continue;
        };
        let is_type =
            matches!(&t.predicate, NamedNodePattern::NamedNode(n) if n.as_str() == RDF_TYPE);
        if is_type {
            if let TermPattern::NamedNode(c) = &t.object {
                if let Some(class) = c.as_str().strip_prefix(MD) {
                    roles.entry(s.clone()).or_default().insert(class);
                }
            }
        }
        let object_role = match local {
            Some("section") => Some("Heading"),
            Some("field") => Some("Field"),
            Some("token") => Some("Token"),
            Some("document") => Some("Document"),
            _ => None,
        };
        if let (Some(role), Some(o)) = (object_role, key(&t.object)) {
            roles.entry(o).or_default().insert(role);
        }
        let subject_role = match local {
            Some("matchedBy") => Some("Match"),
            Some("path" | "field" | "frontmatter" | "lineCount") => Some("Document"),
            Some("key") => Some("Field"),
            _ => None,
        };
        if let Some(role) = subject_role {
            roles.entry(s).or_default().insert(role);
        }
    }
    let role_of = |k: &str| -> String {
        roles
            .get(k)
            .map(|r| r.iter().copied().collect::<Vec<_>>().join("/"))
            .unwrap_or_default()
    };

    // Origins: where each variable's value comes from.
    let mut origins: BTreeMap<String, BTreeSet<Origin>> = BTreeMap::new();
    let mut expects: BTreeSet<Expect> = BTreeSet::new();
    let mut reads_any_citation = false;
    for t in &walk.triples {
        let Some(local) = md_local(&t.predicate) else {
            continue;
        };
        let role = key(&t.subject).map(|s| role_of(&s)).unwrap_or_default();
        let origin = Origin {
            predicate: local.to_string(),
            role,
            computed: false,
            folded: false,
        };
        match &t.object {
            TermPattern::Variable(v) => {
                if local == "matchedBy" {
                    reads_any_citation = true;
                }
                origins
                    .entry(v.as_str().to_string())
                    .or_default()
                    .insert(origin);
            }
            TermPattern::Literal(l) => {
                expects.insert(expect(&origin, l, How::Equals));
            }
            _ => {}
        }
    }
    // Through BIND, to a fixed point: each pass can only add, and there are finitely many
    // (variable, origin) pairs.
    loop {
        let mut changed = false;
        for (var, e) in &walk.extends {
            let (folded, computed) = transform(e);
            let mut add = BTreeSet::new();
            for from in variables(e) {
                for o in origins.get(from).into_iter().flatten() {
                    add.insert(Origin {
                        predicate: o.predicate.clone(),
                        role: o.role.clone(),
                        computed: o.computed || computed,
                        folded: o.folded || folded,
                    });
                }
            }
            let entry = origins.entry(var.to_string()).or_default();
            for o in add {
                changed |= entry.insert(o);
            }
        }
        if !changed {
            break;
        }
    }

    // Comparisons, wherever an expression makes one.
    let compare =
        |vars: &BTreeSet<&str>, how: How, folded: bool, l: &Literal, out: &mut BTreeSet<Expect>| {
            for var in vars {
                for o in origins.get(*var).into_iter().flatten() {
                    let how = if how == How::Equals && (folded || o.folded) {
                        How::EqualsIgnoringCase
                    } else {
                        how
                    };
                    out.insert(expect(o, l, how));
                }
            }
        };
    for e in &walk.expressions {
        let mut stack = vec![*e];
        while let Some(e) = stack.pop() {
            match e {
                Expression::Equal(a, b) | Expression::SameTerm(a, b) => {
                    for (side, other) in [(a, b), (b, a)] {
                        if let Expression::Literal(l) = &**other {
                            compare(
                                &variables(side),
                                How::Equals,
                                transform(side).0,
                                l,
                                &mut expects,
                            );
                        }
                    }
                }
                Expression::In(a, list) => {
                    for item in list {
                        if let Expression::Literal(l) = item {
                            compare(&variables(a), How::Equals, transform(a).0, l, &mut expects);
                        }
                    }
                }
                Expression::FunctionCall(f, args) => {
                    let how = match f {
                        Function::Regex => Some(How::Matches),
                        Function::Contains => Some(How::Contains),
                        Function::StrStarts => Some(How::StartsWith),
                        Function::StrEnds => Some(How::EndsWith),
                        _ => None,
                    };
                    if let (Some(how), [a, Expression::Literal(l), ..]) = (how, args.as_slice()) {
                        compare(&variables(a), how, false, l, &mut expects);
                    }
                }
                _ => {}
            }
            stack.extend(children(e));
        }
    }
    let mut iri_constants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (vars, rows) in &walk.values {
        for row in rows.iter() {
            for (var, cell) in vars.iter().zip(row) {
                match cell {
                    Some(GroundTerm::Literal(l)) => {
                        compare(&BTreeSet::from([*var]), How::Equals, false, l, &mut expects)
                    }
                    Some(GroundTerm::NamedNode(n)) => {
                        iri_constants
                            .entry(var.to_string())
                            .or_default()
                            .insert(n.as_str().to_string());
                    }
                    _ => {}
                }
            }
        }
    }
    for (var, e) in &walk.extends {
        let mut found = BTreeSet::new();
        constants(e, &mut found);
        iri_constants
            .entry(var.to_string())
            .or_default()
            .extend(found);
    }

    ConstructFacts {
        index: 0,
        problem: None,
        expects,
        produces: produces(template, &iri_constants),
        reads_any_citation,
    }
}

fn expect(o: &Origin, l: &Literal, how: How) -> Expect {
    let value = l.value().to_string();
    let heading = o.role.split('/').any(|r| r == "Heading");
    let kind = match (o.predicate.as_str(), o.computed) {
        ("key", false) => ExpectKind::FrontmatterKey,
        ("path", false) => ExpectKind::Path,
        ("matchedBy", false) => ExpectKind::CitationName,
        ("text", false) if heading => ExpectKind::Heading,
        ("level", false) if heading => ExpectKind::HeadingLevel,
        _ => ExpectKind::Value,
    };
    let on = if kind != ExpectKind::Value {
        String::new()
    } else {
        let of = if o.role.is_empty() {
            String::new()
        } else {
            format!(
                " of {}",
                o.role
                    .split('/')
                    .map(|r| format!("md:{r}"))
                    .collect::<Vec<_>>()
                    .join("/")
            )
        };
        let computed = if o.computed { "computed from " } else { "" };
        format!("{computed}md:{}{of}", o.predicate)
    };
    Expect {
        kind,
        value,
        how,
        on,
    }
}

fn produces(
    template: &[TriplePattern],
    iri_constants: &BTreeMap<String, BTreeSet<String>>,
) -> Produces {
    let mut out = Produces::default();
    let resolve = |v: &str| iri_constants.get(v).filter(|c| !c.is_empty());
    for t in template {
        if matches!(t.subject, TermPattern::BlankNode(_))
            || matches!(t.object, TermPattern::BlankNode(_))
        {
            out.blank_node = true;
        }
        let predicate = match &t.predicate {
            NamedNodePattern::NamedNode(n) => Some(n.as_str().to_string()),
            NamedNodePattern::Variable(v) => {
                match resolve(v.as_str()) {
                    Some(c) => out.predicates.extend(c.iter().cloned()),
                    None => out.computed_predicate = true,
                }
                None
            }
        };
        if predicate.as_deref() == Some(RDF_TYPE) {
            match &t.object {
                TermPattern::NamedNode(n) => {
                    out.classes.insert(n.as_str().to_string());
                }
                TermPattern::Variable(v) => match resolve(v.as_str()) {
                    Some(c) => out.classes.extend(c.iter().cloned()),
                    None => out.computed_class = true,
                },
                _ => {}
            }
        }
        if let Some(p) = predicate {
            out.predicates.insert(p);
        }
    }
    out
}

/// The namespaces every report writes short, whatever the mapping declares.
pub(crate) const PREFIXES: [(&str, &str); 5] = [
    ("md", MD),
    ("sh", "http://www.w3.org/ns/shacl#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
];

/// An IRI for display: a short form for the namespaces every mapping uses, else `<iri>`.
///
/// ```text
/// https://ikigai-rs.dev/ns/md#text            → md:text
/// http://www.w3.org/2001/XMLSchema#date       → xsd:date
/// http://example.org/decisions#title          → <http://example.org/decisions#title>
/// ```
pub(crate) fn short(iri: &str) -> String {
    for (prefix, ns) in PREFIXES {
        if let Some(local) = iri.strip_prefix(ns) {
            return format!("{prefix}:{local}");
        }
    }
    format!("<{iri}>")
}
