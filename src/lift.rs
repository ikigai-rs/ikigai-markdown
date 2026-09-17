//! Stage 1: the generic structural lift. Markdown in, a structural graph out.
//!
//! ★ This file must not know what any document is ABOUT. It recognizes CommonMark
//! and GFM constructs — frontmatter, headings, paragraphs, lists, links, code,
//! tables, HTML blocks — and applies the two operations a [`Profile`] asks for by
//! name (split a frontmatter key, scan for a pattern). A branch here that recognizes
//! one project's convention is a bug: that belongs in the mapping.
//!
//! ## The graph
//!
//! Everything lands in one named graph, the document's IRI `D`, and every node is
//! skolemized under it (`D#b3`, `D#fm1`, `D#m12`) — no blank nodes, so two lifts of
//! the same bytes are the same quads.
//!
//! - `D a md:Document ; md:path ; md:source? ; md:lineCount`
//! - frontmatter: `D md:field F`, `F a md:Field ; md:key ; md:value | md:token ;
//!   md:line ; md:order`. Nested keys are dot-joined; a YAML sequence and a profile
//!   split both become `md:token` nodes (`md:text`, `md:index`). Values are the text
//!   the author wrote — `0002` stays `0002`; typing is the mapping's job.
//! - blocks: `md:Heading` (`md:level`), `md:Paragraph`, `md:List` (`md:ordered`),
//!   `md:ListItem` (`md:checked`?), `md:BlockQuote`, `md:CodeBlock` (`md:info`),
//!   `md:HtmlBlock`, `md:Table`, `md:TableRow` (`md:header`), `md:TableCell`
//!   (`md:column`), `md:ThematicBreak`. Every block: `md:document`, `md:order`
//!   (document order), `md:line`, `md:endLine`, `md:parent` (its container, or `D`),
//!   `md:section` (the nearest heading above it; a heading's is its parent heading).
//!   Text-bearing blocks carry `md:text` (plain inline text) and `md:raw` (the
//!   markdown source). A list item's text is its OWN inline content — the nested
//!   list and any HTML block inside it are separate blocks.
//! - `md:Link` / `md:Image`: `md:href`, `md:text`, `md:block`, `md:document`.
//! - `md:Match` (profile citations): `md:matchedBy` (the citation's name), `md:text`,
//!   `md:value` (the named group `v`, else group 1, else the whole match), `md:line`,
//!   `md:block`?, `md:section`?, `md:document`.

use oxigraph::model::{GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use yaml_rust2::parser::{Event as YamlEvent, MarkedEventReceiver, Parser as YamlParser};
use yaml_rust2::scanner::Marker;

use crate::mapping::Profile;
use crate::vocab::{md, RDF_TYPE, XSD_BOOLEAN, XSD_INTEGER};

/// Which document is being lifted.
#[derive(Debug, Clone)]
pub struct DocRef {
    /// The document's IRI — also the name of the graph everything lands in.
    pub iri: NamedNode,
    /// The logical path (`md:path`), which a mapping reads with string functions.
    pub path: String,
    /// The resource the bytes were resolved from, when there was one.
    pub source: Option<NamedNode>,
}

/// Lift `markdown` into its structural graph under `doc`, applying `profile`.
///
/// The lift is total: malformed frontmatter is recorded on the document as
/// `md:frontmatterError` rather than refusing the whole document.
///
/// ```
/// use ikigai_markdown::{lift, DocRef, Profile};
/// use oxigraph::model::NamedNode;
/// let doc = DocRef {
///     iri: NamedNode::new("urn:markdown:doc:a.md").unwrap(),
///     path: "a.md".into(),
///     source: None,
/// };
/// let quads = lift("# Title\n\n- one\n", &doc, &Profile::default());
/// let nq: Vec<String> = quads.iter().map(|q| q.to_string()).collect();
/// assert!(nq.iter().any(|q| q.contains("<https://ikigai-rs.dev/ns/md#text> \"Title\"")));
/// assert!(nq.iter().all(|q| q.ends_with("<urn:markdown:doc:a.md>")));
/// ```
pub fn lift(markdown: &str, doc: &DocRef, profile: &Profile) -> Vec<Quad> {
    let mut out = Out::new(doc);
    let lines = LineIndex::new(markdown);

    out.add(doc.iri.clone(), RDF_TYPE, md("Document"));
    out.lit(doc.iri.clone(), "path", Literal::from(doc.path.as_str()));
    if let Some(source) = &doc.source {
        out.add(doc.iri.clone(), &md("source").into_string(), source.clone());
    }
    out.int(doc.iri.clone(), "lineCount", lines.count() as i64);

    let blocks = structure(markdown, doc, profile, &lines, &mut out);
    citations(markdown, doc, profile, &lines, &blocks, &mut out);
    out.quads
}

// --- emission ----------------------------------------------------------------

struct Out {
    graph: GraphName,
    base: String,
    quads: Vec<Quad>,
}

impl Out {
    fn new(doc: &DocRef) -> Self {
        let iri = doc.iri.as_str();
        // A fragment is appended with `#` unless the IRI already has one.
        let base = if iri.contains('#') {
            format!("{iri}-")
        } else {
            format!("{iri}#")
        };
        Out {
            graph: GraphName::NamedNode(doc.iri.clone()),
            base,
            quads: Vec::new(),
        }
    }

    fn node(&self, local: &str) -> NamedNode {
        NamedNode::new_unchecked(format!("{}{local}", self.base))
    }

    fn add(&mut self, s: NamedNode, p: &str, o: impl Into<Term>) {
        self.quads.push(Quad::new(
            NamedOrBlankNode::NamedNode(s),
            NamedNode::new_unchecked(p),
            o,
            self.graph.clone(),
        ));
    }

    fn rel(&mut self, s: NamedNode, p: &str, o: NamedNode) {
        self.add(s, &md(p).into_string(), o);
    }

    fn lit(&mut self, s: NamedNode, p: &str, o: Literal) {
        self.add(s, &md(p).into_string(), o);
    }

    fn text(&mut self, s: NamedNode, p: &str, o: &str) {
        self.lit(s, p, Literal::from(o));
    }

    fn int(&mut self, s: NamedNode, p: &str, n: i64) {
        let lit = Literal::new_typed_literal(n.to_string(), NamedNode::new_unchecked(XSD_INTEGER));
        self.lit(s, p, lit);
    }

    fn bool(&mut self, s: NamedNode, p: &str, b: bool) {
        let lit = Literal::new_typed_literal(b.to_string(), NamedNode::new_unchecked(XSD_BOOLEAN));
        self.lit(s, p, lit);
    }
}

/// Byte offset → 1-based line number.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        // A trailing newline does not start a line.
        if starts.len() > 1 && *starts.last().unwrap() == text.len() {
            starts.pop();
        }
        LineIndex { starts }
    }

    fn line(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }

    /// The line holding the LAST byte of a `start..end` range.
    fn end_line(&self, start: usize, end: usize) -> usize {
        self.line(end.saturating_sub(1).max(start))
    }

    fn count(&self) -> usize {
        self.starts.len()
    }
}

// --- structure ---------------------------------------------------------------

/// What kind of block a frame is, and whether inline text collects in it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Heading,
    Paragraph,
    List,
    ListItem,
    BlockQuote,
    CodeBlock,
    HtmlBlock,
    Table,
    TableRow,
    TableCell,
    Frontmatter,
}

impl Kind {
    fn class(self) -> &'static str {
        match self {
            Kind::Heading => "Heading",
            Kind::Paragraph => "Paragraph",
            Kind::List => "List",
            Kind::ListItem => "ListItem",
            Kind::BlockQuote => "BlockQuote",
            Kind::CodeBlock => "CodeBlock",
            Kind::HtmlBlock => "HtmlBlock",
            Kind::Table => "Table",
            Kind::TableRow => "TableRow",
            Kind::TableCell => "TableCell",
            Kind::Frontmatter => "Frontmatter",
        }
    }

    /// Whether plain inline text is gathered into `md:text`.
    fn collects(self) -> bool {
        matches!(
            self,
            Kind::Heading
                | Kind::Paragraph
                | Kind::ListItem
                | Kind::TableCell
                | Kind::CodeBlock
                | Kind::HtmlBlock
                | Kind::Frontmatter
        )
    }
}

struct Frame {
    kind: Kind,
    node: NamedNode,
    start: usize,
    text: String,
    /// Source ranges of this block's OWN inline content.
    raw: Vec<(usize, usize)>,
    /// The inline run currently open directly in this frame.
    run: Option<(usize, usize)>,
    /// For a table row: the next cell's column.
    next_column: i64,
}

impl Frame {
    fn flush_run(&mut self) {
        if let Some(run) = self.run.take() {
            self.raw.push(run);
        }
    }
}

/// A block's extent, for placing citation matches.
struct Extent {
    start: usize,
    end: usize,
    depth: usize,
    node: NamedNode,
}

struct Link {
    node: NamedNode,
    class: &'static str,
    href: String,
    text: String,
}

fn structure(
    markdown: &str,
    doc: &DocRef,
    profile: &Profile,
    lines: &LineIndex,
    out: &mut Out,
) -> Extents {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;

    let mut stack: Vec<Frame> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut order: i64 = 0;
    let mut link_count = 0usize;
    let mut extents = Extents::default();

    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let kind = match &tag {
                    Tag::Paragraph => Some(Kind::Paragraph),
                    Tag::Heading { .. } => Some(Kind::Heading),
                    Tag::BlockQuote(_) => Some(Kind::BlockQuote),
                    Tag::CodeBlock(_) => Some(Kind::CodeBlock),
                    Tag::HtmlBlock => Some(Kind::HtmlBlock),
                    Tag::List(_) => Some(Kind::List),
                    Tag::Item => Some(Kind::ListItem),
                    Tag::Table(_) => Some(Kind::Table),
                    Tag::TableHead | Tag::TableRow => Some(Kind::TableRow),
                    Tag::TableCell => Some(Kind::TableCell),
                    Tag::MetadataBlock(_) => Some(Kind::Frontmatter),
                    _ => None,
                };
                if let Some(kind) = kind {
                    if let Some(parent) = stack.last_mut() {
                        parent.flush_run();
                    }
                    let node = if kind == Kind::Frontmatter {
                        out.node("frontmatter")
                    } else {
                        order += 1;
                        out.node(&format!("b{order}"))
                    };
                    if kind != Kind::Frontmatter {
                        let parent = stack
                            .last()
                            .map(|f| f.node.clone())
                            .unwrap_or_else(|| doc.iri.clone());
                        out.add(node.clone(), RDF_TYPE, md(kind.class()));
                        out.rel(node.clone(), "document", doc.iri.clone());
                        out.rel(node.clone(), "parent", parent);
                        out.int(node.clone(), "order", order);
                        out.int(node.clone(), "line", lines.line(range.start) as i64);
                        out.int(
                            node.clone(),
                            "endLine",
                            lines.end_line(range.start, range.end) as i64,
                        );
                        if kind == Kind::Heading {
                            if let Tag::Heading { level, .. } = &tag {
                                let level = *level as i64;
                                extents.open_heading(range.start, level, node.clone());
                                out.int(node.clone(), "level", level);
                            }
                        }
                        if let Some(section) = extents.section_of(&node) {
                            out.rel(node.clone(), "section", section);
                        }
                        extents.blocks.push(Extent {
                            start: range.start,
                            end: range.end,
                            depth: stack.len(),
                            node: node.clone(),
                        });
                    }
                    match &tag {
                        Tag::List(start) => out.bool(node.clone(), "ordered", start.is_some()),
                        Tag::TableHead => out.bool(node.clone(), "header", true),
                        Tag::TableRow => out.bool(node.clone(), "header", false),
                        Tag::CodeBlock(CodeBlockKind::Fenced(info)) => {
                            out.bool(node.clone(), "fenced", true);
                            if !info.is_empty() {
                                out.text(node.clone(), "info", info);
                            }
                        }
                        Tag::CodeBlock(CodeBlockKind::Indented) => {
                            out.bool(node.clone(), "fenced", false)
                        }
                        Tag::TableCell => {
                            if let Some(row) = stack.last_mut() {
                                out.int(node.clone(), "column", row.next_column);
                                row.next_column += 1;
                            }
                        }
                        _ => {}
                    }
                    stack.push(Frame {
                        kind,
                        node,
                        start: range.start,
                        text: String::new(),
                        raw: Vec::new(),
                        run: None,
                        next_column: 0,
                    });
                } else {
                    let link = match tag {
                        Tag::Link { dest_url, .. } => Some(("Link", dest_url)),
                        Tag::Image { dest_url, .. } => Some(("Image", dest_url)),
                        _ => None,
                    };
                    if let Some((class, href)) = link {
                        link_count += 1;
                        links.push(Link {
                            node: out.node(&format!("l{link_count}")),
                            class,
                            href: href.to_string(),
                            text: String::new(),
                        });
                    }
                    inline(&mut stack, range.clone());
                }
            }
            Event::End(end) => {
                let closes_block = matches!(
                    end,
                    TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::BlockQuote(_)
                        | TagEnd::CodeBlock
                        | TagEnd::HtmlBlock
                        | TagEnd::List(_)
                        | TagEnd::Item
                        | TagEnd::Table
                        | TagEnd::TableHead
                        | TagEnd::TableRow
                        | TagEnd::TableCell
                        | TagEnd::MetadataBlock(_)
                );
                if closes_block {
                    let Some(mut frame) = stack.pop() else {
                        continue;
                    };
                    frame.flush_run();
                    finish(markdown, doc, profile, lines, out, &mut stack, frame);
                } else if matches!(end, TagEnd::Link | TagEnd::Image) {
                    if let Some(link) = links.pop() {
                        out.add(link.node.clone(), RDF_TYPE, md(link.class));
                        out.rel(link.node.clone(), "document", doc.iri.clone());
                        out.text(link.node.clone(), "href", &link.href);
                        out.text(link.node.clone(), "text", link.text.trim());
                        if let Some(block) =
                            stack.iter().rev().find(|f| f.kind != Kind::Frontmatter)
                        {
                            out.rel(link.node, "block", block.node.clone());
                        }
                    }
                }
            }
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                push_text(&mut stack, &mut links, &t);
                inline(&mut stack, range);
            }
            Event::Html(t) => {
                // Block-level HTML content: the HtmlBlock's own text.
                push_text(&mut stack, &mut links, &t);
            }
            Event::InlineHtml(_) | Event::FootnoteReference(_) => inline(&mut stack, range),
            Event::SoftBreak | Event::HardBreak => {
                let separator = if stack.last().is_some_and(|f| f.kind == Kind::CodeBlock) {
                    "\n"
                } else {
                    " "
                };
                push_text(&mut stack, &mut links, separator);
                inline(&mut stack, range);
            }
            Event::TaskListMarker(checked) => {
                if let Some(item) = stack.iter().rev().find(|f| f.kind == Kind::ListItem) {
                    out.bool(item.node.clone(), "checked", checked);
                }
            }
            Event::Rule => {
                if let Some(parent) = stack.last_mut() {
                    parent.flush_run();
                }
                order += 1;
                let node = out.node(&format!("b{order}"));
                let parent = stack
                    .last()
                    .map(|f| f.node.clone())
                    .unwrap_or_else(|| doc.iri.clone());
                out.add(node.clone(), RDF_TYPE, md("ThematicBreak"));
                out.rel(node.clone(), "document", doc.iri.clone());
                out.rel(node.clone(), "parent", parent);
                out.int(node.clone(), "order", order);
                out.int(node.clone(), "line", lines.line(range.start) as i64);
                if let Some(section) = extents.section_of(&node) {
                    out.rel(node, "section", section);
                }
            }
        }
    }
    extents
}

/// Extend the open inline run of the innermost frame.
fn inline(stack: &mut [Frame], range: std::ops::Range<usize>) {
    if let Some(frame) = stack.last_mut() {
        frame.run = Some(match frame.run {
            Some((s, e)) => (s.min(range.start), e.max(range.end)),
            None => (range.start, range.end),
        });
    }
}

fn push_text(stack: &mut [Frame], links: &mut [Link], text: &str) {
    if let Some(frame) = stack.last_mut() {
        if frame.kind.collects() {
            frame.text.push_str(text);
        }
    }
    for link in links.iter_mut() {
        link.text.push_str(text);
    }
}

/// Close a frame: emit its text, and hand a paragraph's text up to a list item.
fn finish(
    markdown: &str,
    doc: &DocRef,
    profile: &Profile,
    lines: &LineIndex,
    out: &mut Out,
    stack: &mut [Frame],
    frame: Frame,
) {
    let raw = frame
        .raw
        .iter()
        .map(|&(s, e)| markdown[s..e].trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    match frame.kind {
        Kind::Frontmatter => frontmatter(&frame.text, frame.start, doc, profile, lines, out),
        Kind::CodeBlock | Kind::HtmlBlock => {
            out.text(frame.node.clone(), "text", &frame.text);
        }
        kind if kind.collects() => {
            let text = collapse(&frame.text);
            out.text(frame.node.clone(), "text", &text);
            if !raw.is_empty() {
                out.text(frame.node.clone(), "raw", &raw);
            }
            // A paragraph directly inside a list item is that item's own content
            // (a loose list wraps every item's text in one).
            if kind == Kind::Paragraph {
                if let Some(parent) = stack.last_mut() {
                    if parent.kind == Kind::ListItem {
                        if !parent.text.is_empty() {
                            parent.text.push(' ');
                        }
                        parent.text.push_str(&text);
                        parent.raw.extend(frame.raw.iter().copied());
                    }
                }
            }
        }
        _ => {}
    }
}

/// Runs of whitespace → one space, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --- sections ----------------------------------------------------------------

#[derive(Default)]
struct Extents {
    blocks: Vec<Extent>,
    /// Headings in document order: (start offset, level, node).
    headings: Vec<(usize, i64, NamedNode)>,
    /// The open heading chain, outermost first.
    chain: Vec<(i64, NamedNode)>,
}

impl Extents {
    fn open_heading(&mut self, start: usize, level: i64, node: NamedNode) {
        while self.chain.last().is_some_and(|(l, _)| *l >= level) {
            self.chain.pop();
        }
        self.headings.push((start, level, node));
    }

    /// The section a block just opened belongs to: the innermost open heading —
    /// for a heading, the one it nests under (it is pushed after this is read).
    fn section_of(&mut self, node: &NamedNode) -> Option<NamedNode> {
        let section = self.chain.last().map(|(_, n)| n.clone());
        if let Some((_, level, heading)) = self.headings.last() {
            if heading == node {
                self.chain.push((*level, heading.clone()));
            }
        }
        section
    }

    /// The nearest heading at or before `offset`.
    fn section_at(&self, offset: usize) -> Option<NamedNode> {
        let i = self.headings.partition_point(|(s, _, _)| *s <= offset);
        i.checked_sub(1).map(|i| self.headings[i].2.clone())
    }

    /// The innermost block containing `offset`.
    fn block_at(&self, offset: usize) -> Option<NamedNode> {
        self.blocks
            .iter()
            .filter(|b| b.start <= offset && offset < b.end)
            .max_by_key(|b| (b.depth, b.start))
            .map(|b| b.node.clone())
    }
}

// --- frontmatter --------------------------------------------------------------

struct Field {
    key: String,
    values: Vec<String>,
    sequence: bool,
    line: usize,
}

enum Ctx {
    Map {
        prefix: String,
        pending: Option<(String, usize)>,
    },
    Seq {
        field: usize,
        prefix: String,
    },
}

#[derive(Default)]
struct YamlFields {
    stack: Vec<Ctx>,
    fields: Vec<Field>,
}

fn join_key(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

impl YamlFields {
    /// The key a nested container opens under, consuming a pending map key.
    fn open_key(&mut self, line: usize) -> (String, usize) {
        match self.stack.last_mut() {
            Some(Ctx::Map { prefix, pending }) => match pending.take() {
                Some((key, l)) => (join_key(prefix, &key), l),
                None => (prefix.clone(), line),
            },
            Some(Ctx::Seq { prefix, .. }) => (prefix.clone(), line),
            None => (String::new(), line),
        }
    }
}

impl MarkedEventReceiver for YamlFields {
    fn on_event(&mut self, ev: YamlEvent, mark: Marker) {
        let line = mark.line();
        match ev {
            YamlEvent::Scalar(value, ..) => match self.stack.last_mut() {
                Some(Ctx::Map { prefix, pending }) => match pending.take() {
                    None => *pending = Some((value, line)),
                    Some((key, l)) => self.fields.push(Field {
                        key: join_key(prefix, &key),
                        values: vec![value],
                        sequence: false,
                        line: l,
                    }),
                },
                Some(Ctx::Seq { field, .. }) => self.fields[*field].values.push(value),
                None => self.fields.push(Field {
                    key: String::new(),
                    values: vec![value],
                    sequence: false,
                    line,
                }),
            },
            YamlEvent::MappingStart(..) => {
                let (prefix, _) = self.open_key(line);
                self.stack.push(Ctx::Map {
                    prefix,
                    pending: None,
                });
            }
            YamlEvent::SequenceStart(..) => {
                let field = match self.stack.last() {
                    Some(Ctx::Seq { field, .. }) => Some(*field),
                    _ => None,
                };
                let (prefix, l) = self.open_key(line);
                let field = field.unwrap_or_else(|| {
                    self.fields.push(Field {
                        key: prefix.clone(),
                        values: Vec::new(),
                        sequence: true,
                        line: l,
                    });
                    self.fields.len() - 1
                });
                self.stack.push(Ctx::Seq { field, prefix });
            }
            YamlEvent::MappingEnd | YamlEvent::SequenceEnd => {
                self.stack.pop();
            }
            _ => {}
        }
    }
}

fn frontmatter(
    yaml: &str,
    start: usize,
    doc: &DocRef,
    profile: &Profile,
    lines: &LineIndex,
    out: &mut Out,
) {
    let fm = out.node("frontmatter");
    out.add(fm.clone(), RDF_TYPE, md("Frontmatter"));
    out.rel(fm.clone(), "document", doc.iri.clone());
    out.text(fm.clone(), "text", yaml);
    out.rel(doc.iri.clone(), "frontmatter", fm);

    let mut receiver = YamlFields::default();
    if let Err(e) = YamlParser::new_from_str(yaml).load(&mut receiver, false) {
        out.text(doc.iri.clone(), "frontmatterError", &e.to_string());
        return;
    }
    // The frontmatter block's text starts on the line after the opening `---`; YAML
    // lines are 1-based within that text.
    let first = lines.line(start) + 1;
    for (i, field) in receiver.fields.iter().enumerate() {
        let node = out.node(&format!("fm{}", i + 1));
        out.rel(doc.iri.clone(), "field", node.clone());
        out.add(node.clone(), RDF_TYPE, md("Field"));
        out.rel(node.clone(), "document", doc.iri.clone());
        out.text(node.clone(), "key", &field.key);
        out.int(node.clone(), "order", i as i64 + 1);
        out.int(
            node.clone(),
            "line",
            (first + field.line.saturating_sub(1)) as i64,
        );

        let mut tokens: Vec<String> = Vec::new();
        if field.sequence {
            tokens.extend(field.values.iter().cloned());
        } else {
            for value in &field.values {
                out.text(node.clone(), "value", value);
            }
            if let Some(split) = profile.splits.iter().find(|s| s.key == field.key) {
                for value in &field.values {
                    tokens.extend(
                        split
                            .delimiter
                            .split(value)
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(str::to_string),
                    );
                }
            }
        }
        for (j, token) in tokens.iter().enumerate() {
            let t = out.node(&format!("fm{}-t{}", i + 1, j + 1));
            out.rel(node.clone(), "token", t.clone());
            out.add(t.clone(), RDF_TYPE, md("Token"));
            out.text(t.clone(), "text", token);
            out.int(t, "index", j as i64 + 1);
        }
    }
}

// --- citations ----------------------------------------------------------------

fn citations(
    markdown: &str,
    doc: &DocRef,
    profile: &Profile,
    lines: &LineIndex,
    extents: &Extents,
    out: &mut Out,
) {
    let mut count = 0usize;
    for citation in &profile.citations {
        for caps in citation.pattern.captures_iter(markdown) {
            let whole = caps.get(0).expect("group 0 always matches");
            if whole.as_str().is_empty() {
                continue;
            }
            count += 1;
            let node = out.node(&format!("m{count}"));
            let value = caps
                .name("v")
                .or_else(|| caps.get(1))
                .map_or(whole.as_str(), |g| g.as_str());
            out.add(node.clone(), RDF_TYPE, md("Match"));
            out.rel(node.clone(), "document", doc.iri.clone());
            out.text(node.clone(), "matchedBy", &citation.name);
            out.text(node.clone(), "text", whole.as_str());
            out.text(node.clone(), "value", value);
            out.int(node.clone(), "line", lines.line(whole.start()) as i64);
            if let Some(block) = extents.block_at(whole.start()) {
                out.rel(node.clone(), "block", block);
            }
            if let Some(section) = extents.section_at(whole.start()) {
                out.rel(node, "section", section);
            }
        }
    }
}
