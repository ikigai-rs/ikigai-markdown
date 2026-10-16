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
  union is the domain graph. They must not emit blank nodes, so build IRIs with
  `IRI(CONCAT(…))`.
- **`md:split` and `md:citation`** make up the **lift profile**. It exists because of
  one wall: SPARQL 1.1 cannot split a string into rows (oxigraph adds no function for
  it, and xrust has no `fn:tokenize`). Joining a regex against the entities that exist
  finds every reference to something that *does* exist, and can never find one to
  something that doesn't. So stage 1 applies the profile generically. It splits the
  named keys and scans for the named patterns, and every piece becomes a node. A
  dangling reference is then just data.

Three teams with three document conventions are three mapping files and zero lines of
code. `urn:markdown:vocabulary` serves the `md:` terms.

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
  with a transient `Unavailable`. A host sets its own with `space_with_budget`. On wasm
  there are no threads, and no deadline.
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

The served mapping is cached under the golden thread `urn:file:<absolute path>` — the
name a filesystem watcher cuts — so editing the installed file re-lifts every document
under it. A name with no `mapping.ttl` behind it is a **not-found** naming the path it
looked at; there is no fallback to another mapping, and none to an unmapped lift.

A host mounts `ikigai_markdown::space()` for this machine's config home, or
`space_with(Arc::new(MappingHome::new(Some(home))))` to serve another (`home` is a
`PathBuf` naming the `ikigai` directory itself; `None` is a host with no config home,
where a named mapping is a not-found). On wasm there is no config home and the
endpoint is simply not bound. `space_with_budget(home, budget)` also sets the lifts'
time budget (`ikigai_markdown::EvaluationBudget`, re-exported from `ikigai-store`): make
one per host and pass it to every space you mount, since the cap on evaluations still
running past their budget is counted per budget.

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

## Linting a mapping, and a corpus under it

When a document format changes, a mapping can stop lifting some documents the way it
used to, and nothing fails: the lift still answers, with less in it. The linter says
what changed. It is four resources, each with a text face (the default) and a Turtle face
(`as=text/turtle`), so it runs in any host that mounts the space:

| resource | what it answers |
|---|---|
| `urn:markdown:lint:mapping mapping=…` | The mapping on its own. Every `md:split` and `md:citation` pattern compiles as a Rust regex, every `md:construct` parses and is a CONSTRUCT within the lift's bounds, and nothing writes a blank node. What the constructs write is checked against the shape the mapping states with `md:shape`. With no shape, the report says so. |
| `urn:markdown:lint:expects mapping=…` | What the mapping depends on in a document, for the person writing the documents: the paths, frontmatter keys, headings, heading levels and citation forms it reads, and every other literal its constructs compare document text with. Read straight from the constructs' SPARQL algebra, with nothing run. |
| `urn:markdown:lint:snapshot docs=… root=… mapping=…` | A list of documents (one IRI per line, so a list can be piped in) lifted and reduced to what the corpus check compares, as sorted N-Quads with one graph per document. Save it, and it is a baseline. |
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
For a corpus larger than these, lint it in parts.

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
