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
- **Two regex dialects.** `md:pattern` is a Rust `regex` (stage 1): named groups and
  `(?m)` work there. `REGEX`/`REPLACE` inside a construct use SPARQL's XPath flavour.
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
`space_with(MappingHome::new(home))` to serve another. On wasm there is no config home
and the endpoint is simply not bound.

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

## Not here yet

md→html, an HTML face, transreptor registration, and any write path.
