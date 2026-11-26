# ikigai-markdown

Markdown as a graph for [ikigai](https://github.com/ikigai-rs): `urn:markdown:lift`.

```text
source urn:file:docs/notes/0007.md | urn:markdown:lift mapping=urn:file:notes.mapping.ttl
source urn:markdown:lift src=urn:file:notes/today.md as=application/trig
source urn:file:notes/0007.md | urn:markdown:lift mapping=urn:markdown:mapping:notes
```

The file stays the thing a person edits. The graph is **derived** from it, never
written back.

## One generic lift, and everything specific as mapping

**Stage 1, the structural lift, is the only code.** It turns markdown into a graph of
what the document *is*: frontmatter fields, headings as sections with order and
containment, list items with their own text, paragraphs, links, code blocks, tables,
HTML blocks. It does not know what any document is *about*. A test
(`no_source_file_knows_a_document_process_vocabulary`) fails if a process word shows
up anywhere in `src/`.

**Stage 2, the mapping, is a resource.** A mapping is one Turtle file:

```turtle
@prefix md: <https://ikigai-rs.dev/ns/md#> .
<#m> a md:Mapping ;
    md:split    [ md:key "tags" ; md:delimiter "," ] ;               # a list-valued frontmatter key
    md:citation [ md:name "issue" ; md:pattern "#(?P<v>[0-9]+)" ] ;  # a pattern to emit as tokens
    md:construct """PREFIX md: <https://ikigai-rs.dev/ns/md#>
      CONSTRUCT { ?d <http://purl.org/dc/terms/title> ?t }
      WHERE { ?h a md:Heading ; md:document ?d ; md:level 1 ; md:text ?t }""" .
```

- **`md:construct`**: SPARQL CONSTRUCTs run over the document's structural graph. Their
  union is the domain graph. They must be repeatable and must not emit blank nodes.
  Build IRIs from stable input keys.
- **`md:split` and `md:citation`** make up the **lift profile**. It exists because of
  one wall: SPARQL 1.1 cannot split a string into rows (oxigraph adds no function for
  it, and xrust has no `fn:tokenize`). Joining a regex against the entities that exist
  finds every reference to something that *does* exist, and can never find one to
  something that doesn't. So stage 1 applies the profile generically. It splits the
  named keys and scans for the named patterns, and every piece becomes a node. A
  dangling reference is then just data.

Three teams with three document conventions are three mapping files and zero lines of
code. `urn:markdown:vocabulary` serves the `md:` terms.

Frontmatter is the YAML block that opens a document, on its first line. A `---`, text,
`---` run later in the document is what CommonMark makes of it, a thematic break and a
setext heading, never a second frontmatter. Frontmatter keeps scalar text without inferring types, including scalar aliases
(`copy: *anchor`). Collection aliases are not expanded: they produce an
`md:frontmatterError` asking you to write the collection at each use. No partial
fields are emitted on a frontmatter error; the remaining Markdown still lifts.

Ordered lists retain their first number as `md:start` (unordered lists have none).
Every table cell carries its column's `md:alignment`: `none`, `left`, `center`, or
`right`, including header cells. Line counts and locations recognize LF, CRLF, and
lone CR line endings. As defined by the vocabulary, inline `md:text` collapses
whitespace, including link labels and image descriptions; it is not a byte-for-byte
copy of the source.

Structural node IRIs use document order (`#b1`, `#b2`, and so on). Identical input
produces identical IRIs and no blank nodes, but inserting an earlier block can
renumber later nodes. Mappings that need identities across edits should mint domain
IRIs from document keys or content rather than reuse structural block IRIs.

## Migrating mappings that generate identifiers

The lift refuses a nonrepeatable construct before running it, and mapping lint reports
the same reason. An existing mapping that calls `UUID()`, `STRUUID()`, or `BNODE()`
(including `BNODE("label")`), or creates a blank node in the CONSTRUCT template, must
use a stable IRI instead. The repeatability check also rejects `NOW()`, `RAND()`,
`LIMIT` or `OFFSET` even with `ORDER BY`, `SAMPLE`, `GROUP_CONCAT`, and `REDUCED`.
If the classifier reaches its traversal limit, simplify the query before trying again.

For example, if a heading mapping uses `BIND(UUID() AS ?id)` or
`BIND(BNODE(STR(?heading)) AS ?id)`, replace that BIND with an IRI built from the
document IRI and the heading's position. If the template uses `[]` as the subject,
change it to `?id` and add the same BIND.

```sparql
PREFIX md: <https://ikigai-rs.dev/ns/md#>
CONSTRUCT { ?id <https://example.org/vocab/label> ?label }
WHERE {
  ?heading a md:Heading ; md:document ?doc ; md:order ?position ; md:text ?label .
  BIND(IRI(CONCAT("https://example.org/id/heading/",
                  ENCODE_FOR_URI(STR(?doc)), "/", STR(?position))) AS ?id)
}
```

A position identifies the same node for identical inputs, but can change if you add
an earlier block. If the heading's text defines its identity, use a content digest
instead. In the same query, replace the BIND with this one.

```sparql
BIND(IRI(CONCAT("https://example.org/id/heading/",
                ENCODE_FOR_URI(STR(?doc)), "/", SHA256(STR(?label)))) AS ?id)
```

Equal text in one document now shares an ID, and editing that text changes the ID.
If equal headings must stay distinct, include a stable section path or another unique
key from the document. Use an IRI namespace you control. Existing stored IDs are not
rewritten automatically, so update downstream references when replacing them.

## Caching: two golden threads

A lift is a pure function of **(document bytes, mapping bytes)**. `src` and `mapping`
each accept either a resource IRI, resolved through the kernel, or the content by
value. By reference, the kernel folds in each resource's golden thread. A lift over a
watched file and a watched mapping is cached, and it recomputes when either one is
cut. Editing the mapping re-lifts every document under it with no rebuild.
`tests/threads.rs` holds both halves.

## Arguments

| arg | | |
|-----|---|---|
| `src` | required, pipeable | a resource IRI, or the markdown itself. A value that is one token shaped like an absolute IRI is read as a reference. |
| `content` | optional | the markdown by value |
| `mapping` | optional | a mapping IRI (`urn:markdown:mapping:{name}` for one installed in the config home), or Turtle by value. When omitted, you get the structural graph only. |
| `path` | optional | the document's logical path (`md:path`). It names the graph `urn:markdown:doc:{path}`. When omitted, a by-reference document's graph is its own IRI. |
| `as` | optional | `application/n-quads` (default, sorted) or `application/trig` |

## Writing a mapping: traps that cost a round trip

- **Backslashes.** A backslash in a regex, inside a SPARQL string, inside a Turtle
  string, has to be written four times. Write `[0-9]` and `[.]` instead. To match
  whitespace, use a SPARQL long string (`'''[ \t\n]+'''`): Turtle turns `\t\n` into
  raw characters, and a SPARQL short string can't hold raw characters.
- **UNION scoping.** A `FILTER` inside a `UNION` branch only sees that branch's
  variables. If you bind `?path` once outside the branches, the filters silently match
  nothing.
- **OPTIONAL scoping.** An `OPTIONAL` group is evaluated on its own and joined
  afterwards, so it cannot see a variable bound outside it.
  `?h md:text ?t OPTIONAL { BIND(IRI(CONCAT("urn:ex:", STR(?t))) AS ?x) }` leaves `?x`
  unbound on every row, and every template triple that uses `?x` silently disappears.
  It is SPARQL's scoping, not a bug, and the same as the UNION trap above. Put the
  `BIND` outside the `OPTIONAL`, or move the pattern that binds `?t` inside it.
- **Two regex dialects.** `md:pattern` is a Rust `regex` (stage 1): named groups and
  `(?m)` work there. `REGEX`/`REPLACE` inside a construct use SPARQL's XPath flavor.
- **Unbound on purpose.** `BIND(IF(cond, value, ?unbound) AS ?x)` leaves `?x` unbound
  when `cond` is false. A CONSTRUCT template triple with an unbound variable is simply
  not emitted, and that's how one construct writes different predicates per row.
- **A union of graphs is a multiset — count with `DISTINCT`.** Each document lifts into
  its own named graph, so a query over the union (what `reconcile` and the tests run)
  sees a triple once *per graph that asserts it*. A construct that states a fact about
  something shared — a project, a release, anything many documents mention — restates it
  from every one of them, and `COUNT(*)` then counts documents rather than things. It
  does not error and the number looks plausible: counting one corpus's changes this way
  gave 1263 for a true 496. Aggregate with `COUNT(DISTINCT ?x)`, and reach for
  `SELECT DISTINCT` in a subquery before summing.
- **A construct is bounded, because a mapping is caller text.** The bounds are
  `ikigai-store`'s, so they read the same everywhere: at most 1 MiB, nesting at most 64
  brackets (or a run of `!`), at most 32 patterns in one group (a sequence path counts
  one a step), and at most 1024 algebra operators in all. Past any of them the lift is
  refused with an `InvalidArgument` on `mapping` naming the bound, before the query is
  parsed or planned. A real mapping is far inside all four. The bounds keep a construct
  from overflowing the stack and aborting the host, or planning for minutes.
- **A lift answers within a time budget.** What those bounds leave is a cross product of
  unrelated patterns, which over an ordinary document can run for minutes, so a lift's
  constructs run under `ikigai-store`'s `EvaluationBudget`, with the budget chosen as the
  store's doors choose it: **5 s** for every caller, raised by a
  `urn:cap:store:budget:<ms>` grant, up to the ceiling (**120 s**), which root gets. It
  covers all of one lift's constructs together, from when its inputs have resolved. Past
  it the caller gets a typed `Timeout`. ⚠ oxigraph cannot stop a cross product partway,
  so such an evaluation keeps a core busy after its caller was answered; while a quarter
  of the machine's cores are held that way, a lift that needs SPARQL is refused at once
  with a transient `Unavailable`. A host sets its own with `space_with_budget`, or
  `space_under_budget` on any target. On wasm there are no threads, so a construct runs
  inline and cannot be stopped partway; the budget is read on the kernel's injected clock
  (`inv.now()`), and a lift or snapshot that has spent it is refused before its next
  construct or document. A wasm host that injects no clock gets no deadline.
- **A construct reads only the document: no `SERVICE`.** A `SERVICE` anywhere in a
  construct (inside `EXISTS` or `OPTIONAL`, `SILENT`, or with a variable name) is refused
  with an `InvalidArgument` on `mapping` before anything is evaluated, in every build. In
  a host whose dependency graph turns on oxigraph's HTTP client, it would otherwise be an
  outbound request that no network capability gates. To join remote data, fetch it
  through the kernel, where the net capability applies.

## Finding a mapping by name

A mapping does not have to be a path you carry around. Install one in the **config
home** and refer to it by name:

```text
<config home>/markdown/mappings/<name>/mapping.ttl
<config home>/markdown/mappings/<name>/queries/*.rq     # optional, for tools that run them
```

The config home is ikigai's: `$XDG_CONFIG_HOME/ikigai`, or `~/.config/ikigai` when
that is unset. `urn:markdown:mapping:{name}` serves the file there, so a lift names it
like any other resource:

```text
source urn:file:decisions/0007-x.md | urn:markdown:lift mapping=urn:markdown:mapping:example
```

**The module opens no file.** It reads the mapping BY REQUEST from
`urn:markdown:file:mappings/{name}/mapping.ttl`, and the host decides what answers there:
it binds a file module at `urn:markdown:file:{path}` jailed to the config home's
`markdown` directory (`MappingHome::root`). With `ikigai-fs`:

```rust,ignore
let home = Arc::new(MappingHome::new(Some(home_dir)));
let space = ikigai_markdown::space_with(home.clone()).bind(
    UriTemplate::parse(ikigai_markdown::FILE_TEMPLATE)?,
    ikigai_fs::FileEndpoint::new(home.root().unwrap()).cacheable(),
);
```

So reading a named mapping takes the authority reading the file takes, and the caller
holds it: the endpoint declares `urn:cap:fs:read:*`, and `ikigai-fs` checks a
`urn:cap:fs:read:` grant over the mappings directory. A host that binds nothing there
serves no named mapping; the name is a **not-found** that says so.

With a cacheable mount the served mapping is cached under the file's golden thread,
`urn:markdown:file:mappings/{name}/mapping.ttl`, which a write through the kernel cuts,
and a host's watcher over the mount cuts for an edit made outside ikigai, so editing the
installed file re-lifts every document under it. A name with no `mapping.ttl` behind it
is a **not-found** naming the IRI it asked for; there is no fallback to another mapping,
and none to an unmapped lift.

A host mounts `ikigai_markdown::space()` for this machine's config home, or
`space_with(Arc::new(MappingHome::new(Some(home))))` to serve another (`home` is a
`PathBuf` naming the `ikigai` directory itself; `None` is a host with no config home,
where a named mapping is a not-found), and binds the file mount beside either. On wasm
there is no config home and the endpoint is simply not bound.
`space_with_budget(home, budget)` also sets the lifts'
time budget (`ikigai_markdown::EvaluationBudget`, re-exported from `ikigai-store`): make
one per host and pass it to every space you mount, since the cap on evaluations still
running past their budget is counted per budget. `space_under_budget(budget)` is the same
over this machine's config home, and the only way to set a budget on wasm.

## `mappings/example`: a worked mapping

`mappings/example/` is a complete mapping for an invented decision-record format —
`decisions/NNNN-slug.md`, record fields written EITHER as YAML frontmatter or as a
`## Record` bullet list, a comma-delimited `related:` field, and `DR-0007` citations in
prose. It is the thing to read (and copy) when writing your own; `tests/example_mapping.rs`
runs it over `tests/fixtures/example` and pins what it produces, including a dangling
reference that only the profile's split makes visible.

Install it and run its queries over a corpus (read-only; nothing is written):

```text
cp -R mappings/example ~/.config/ikigai/markdown/mappings/example
cargo run --release --example reconcile -- tests/fixtures/example example
```

`reconcile` also takes a mapping FILE instead of a name, with its queries beside it:

```text
cargo run --release --example reconcile -- tests/fixtures/example mappings/example/mapping.ttl
```

## Dialect mappings: `lectern` and `mdbook`

Two mappings for real formats ship beside the example. Each is a mapping file only,
installed by name like any other (`cp -R mappings/lectern ~/.config/ikigai/markdown/mappings/`):

- **`urn:markdown:mapping:mdbook`** (`mappings/mdbook/`) lifts an mdBook's source. Every
  Markdown file except `SUMMARY.md` is a chapter, `urn:mdbook:chapter:path:{path}`, titled
  by its first level-1 heading. `SUMMARY.md` lists chapters by the same IRI, with the text
  each is listed as, its 1-based position, whether it is numbered, and the chapter it is
  nested under. Lift each file with its path from the book's root (`src/SUMMARY.md`).
- **`urn:markdown:mapping:lectern`** (`mappings/lectern/`) lifts
  [Lectern Markdown](https://github.com/bsletten/lectern-slides) slide decks, one deck file
  per document:
  - **Slides.** Slides split the way lectern's fence-aware splitter splits them: at a line
    that is `---` once stripped, outside fenced code and frontmatter. They are numbered
    from 1 within the file, empty slides included, so
    `urn:lectern:slide:path:{path}:number:{n}` is the slide an include range names.
  - **Notes.** Speaker notes (`<!-- notes -->`) and presenter notes
    (`<!-- notes:presenter -->`) end at `<!-- /notes -->` or at the end of the slide. Each
    links the blocks it holds.
  - **Includes.** Each `<!-- include: path#1-3,14 -->` keeps its target and its ranges as
    written, plus each part of the range list (up to 16 parts) as a first and last slide.

  Lectern's fenced-div notes (`::: notes`) and its `Note:` lines are not lifted yet, and
  include targets are not resolved against lectern's partials search path.

Both lift the course directives below. `tests/dialects.rs` pins each fixture's graph
(`tests/fixtures/lectern`, `tests/fixtures/mdbook`).

## Course directives: questions and exercises

A course unit can be a Book chapter or a lectern deck. The same two directives work in
both, written as HTML comments so the file stays CommonMark and nothing shows when it is
rendered. A directive is one HTML block. Its first line is `<!-- question` or
`<!-- exercise`, and then comes one `key: value` per line, then `-->`:

```markdown
<!-- question
name: triple-parts
unit: rdf-basics
kind: choice
prompt: Which three parts make a triple?
choice: [x] subject, predicate, object
choice: [ ] subject, verb, object
-->

<!-- exercise
name: load-a-graph
unit: rdf-basics
path: exercises/load-a-graph
title: Load a graph and count its triples
-->
```

| key | directive | |
|---|---|---|
| `name` | both, required | the key within the unit; with `unit`, the IRI |
| `unit` | both, required | the unit it practices: `urn:learn:unit:name:{unit}` |
| `kind` | question, required | `choice`, `exact` (the response must equal an answer), `short` (a short answer, checked against a model answer) or `check` (a script checks it) |
| `prompt` | question, required | the question, on one line |
| `choice` | question, `choice` kind | one per line, in order; `[x]` marks a correct one, `[ ]` or no marker one that is not |
| `answer` | question, `exact` and `short` | one per line: each accepted answer, or the model answer |
| `check` | question, `check` kind | the path of the script that checks a response |
| `path` | exercise, optional | where the exercise lives |
| `title` | exercise, optional | its title |

A question is `urn:learn:question:unit:{unit}:name:{name}` and its choices are
`…:index:{i}` under `urn:learn:choice:unit:{unit}:question:{name}`. An exercise is
`urn:learn:exercise:unit:{unit}:name:{name}`. The slide or chapter it sits on links it
with `learn:question` or `learn:exercise`. Names and units are percent-encoded in IRIs,
so keep them to lowercase words and hyphens. A directive with no `name` or no `unit` is
not lifted, and neither is one inside fenced code, so a page can show the syntax. The
same question written in two places is one question, so give each its own name.

The terms are `https://ikigai-rs.dev/ns/learn#`, `…/ns/lectern#` and `…/ns/mdbook#`.
Like `md:`, they are not deployed there yet.

## Linting a mapping, and a corpus under it

When a document format changes, a mapping can stop lifting some documents the way it
used to, and nothing fails: the lift still answers, with less in it. The linter says
what changed. It is four resources, each with a text face (the default) and a Turtle face
(`as=text/turtle`), so it runs in any host that mounts the space:

| resource | what it answers |
|---|---|
| `urn:markdown:lint:mapping mapping=…` | The mapping on its own. Every `md:split` and `md:citation` pattern compiles as a Rust regex, every `md:construct` parses and is a repeatable CONSTRUCT within the lift's bounds. What the constructs write is checked against the shape the mapping states with `md:shape`. With no shape, the report says so. |
| `urn:markdown:lint:expects mapping=…` | What the mapping depends on in a document, for the person writing the documents: the paths, frontmatter keys, headings, heading levels and citation forms it reads, and every other literal its constructs compare document text with. Read straight from the constructs' SPARQL algebra, with nothing run. |
| `urn:markdown:lint:snapshot docs=… root=… mapping=…` | A list of documents (one IRI per line, so a list can be piped in) lifted and reduced to what the corpus check compares, as N-Quads with one graph per document, in path order and sorted within each document. Save it, and it is a baseline. |
| `urn:markdown:lint:corpus docs=… root=… mapping=… baseline=…` | The documents lifted now, compared per document with a baseline snapshot. It reports records and fields lost, gained or changed, citations no longer recognized, headings the mapping reads that vanished or moved, frontmatter keys the mapping does not read, frontmatter that no longer parses, documents the lift cannot read, and values that break the stated shape. The output is sorted and deterministic. |

`root` is an IRI prefix the documents share. What follows it is each document's path, and
its graph is `urn:markdown:doc:{path}`, so a snapshot taken on one machine compares with a
corpus on another. `ignore=` takes predicates (full IRIs, or prefixed names in the
mapping's own prefixes) to leave out of the record comparison. Use it for a line number
that a construct copies onto every record and that moves whenever a line is added above.

**The baseline is a snapshot, not a git revision.** A snapshot is a resource like any
other, so it is read through the kernel and keeps its golden thread. It needs no
repository, and it records what the lift actually produced then. To compare with an
earlier revision, take a snapshot of a copy of that revision.

A finding has a severity and a stable code (`heading-vanished`, `citation-lost`,
`field-lost`, `key-unknown`, `shape-violation`, …). An **error** means something the
mapping relied on no longer holds, a **warning** deserves a look, and a **note** is a
change that cannot break anything. A mapping that does not load is reported as a finding,
not refused, because "this input is wrong" is the answer a linter exists to give.

**The shape** is SHACL, written in the mapping itself: `md:shape` names node shapes. The
linter checks a subset of SHACL Core and reports the rest as not checked: `sh:targetClass`,
and on each `sh:property` with a single-predicate `sh:path`, `sh:minCount`, `sh:maxCount`,
`sh:datatype`, `sh:nodeKind`, `sh:class` and `sh:in`. `mappings/example` states one.

**Bounds.** A corpus is caller input, so every dimension is bounded, and past a bound the
request is refused with the bound's name. It is never answered for part of the corpus:
`MAX_DOCUMENTS` (10,000), `MAX_DOCUMENT_BYTES` (4 MiB), `MAX_CORPUS_BYTES` (64 MiB of
documents), `MAX_SNAPSHOT_BYTES` (256 MiB, for the snapshot taken and for a baseline), and
the caller's time budget, the same one a lift runs under, checked before each document.
For a corpus larger than these, lint it in parts. The snapshot appends one reduced document
block at a time; it holds the answer's bytes and one document's lift, quads and sorted lines.
The kernel may also cache lifts and source documents under its host's cache policy (the
kernel's default cache limit is 64 MiB). The 256 MiB snapshot limit remains a byte limit,
not a process memory limit; it has not been raised. The corpus check reads each snapshot one
document at a time, so beyond the snapshots' own bytes it holds the largest document, plus,
when the mapping states a shape, the triples that shape reads.

From a terminal, `examples/lint.rs` wraps all four over files on disk:

```text
cargo run --release --example lint -- mapping  mappings/example/mapping.ttl
cargo run --release --example lint -- expects  example
cargo run --release --example lint -- snapshot tests/fixtures/example example > before.nq
#   … edit the documents …
cargo run --release --example lint -- corpus   tests/fixtures/example example --baseline before.nq
```

It exits 1 when a text report holds an error.

## Not here yet

md→html, an HTML face, transreptor registration, and any write path.

## Accessibility

What this crate emits that people read (the linter's text reports, graphs, and error messages),
the rules that output follows, how it is tested and how to report a barrier are in
[ACCESSIBILITY.md](ACCESSIBILITY.md).
