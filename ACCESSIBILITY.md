# Accessibility

What `ikigai-markdown` emits that people read, the rules that output follows, how it is tested,
and how to report a barrier. It writes graphs (N-Quads, TriG, Turtle), the mapping linter's text
reports, and the text of its errors; it reads markdown and writes no HTML. It is not a conformance
claim. Adapted from the [ACCESSIBILITY.md template](https://github.com/mgifford/ACCESSIBILITY.md)
(MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text only |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-markdown/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-markdown/security/advisories/new) |
| Last reviewed | 2026-10-31 |

## What it emits that people read

- **The linter's text reports** (the default face of `urn:markdown:lint:mapping`,
  `urn:markdown:lint:expects` and `urn:markdown:lint:corpus`; `Report::to_text` and
  `Expectations::to_text` in `src/lint.rs`, used by `src/corpus.rs`): a header saying what was
  checked, a tally (`result: 1 error, 0 warnings, 1 note`), then one line per finding, grouped
  by document, with its severity, a stable code and a one-line message.
- **Graphs**: the lift's N-Quads or TriG (`src/lib.rs`, `src/lift.rs`), the linter's Turtle
  faces, and snapshots. They are data for programs that people also read.
- **Error and refusal messages**: the `InvalidArgument`, `Timeout` and other errors in
  `src/lib.rs`, `src/named.rs`, `src/sparql.rs` and `src/corpus.rs`, which a host shows to
  whoever made the request.
- **The examples** (`examples/lift.rs`, `examples/lint.rs`, `examples/reconcile.rs`) print to a
  terminal: answers and reports on standard output, usage and failures on standard error, with an
  exit status (`lint` exits 1 when a report holds an error, 2 on a usage or input problem). They
  are developer tools, not a shipped binary.

Not this repository's: the page, editor or terminal a host shows any of this in.

## The rules that output follows

Followed by reading the source:

- Severity is a word on every finding (`error`, `warning`, `info`), never only a color or a
  symbol, and the tally spells out its counts with singular and plural forms.
- A finding is one line: a parser's multi-line message is joined (`Finding::new`), so a reader
  moving line by line meets one finding per line.
- IRIs in finding messages are abbreviated with the mapping's own prefixes
  (`Report::abbreviate`), so a report reads in the mapping author's words.
- The report is sorted, so the same inputs give the same text.
- A mapping that does not load is a finding in the report, not a refusal.
- A refusal names the argument (`src`, `as`, `mapping`, `name`, `path`) and says what was
  expected; a missing named mapping names the path that was looked at; the time-budget refusals
  say what to do (narrow the construct, or ask the host for a larger budget).
- Nothing is written in color, by the endpoints or by the examples.

## How it is tested

- `tests/lint.rs` pins the text face byte for byte or by its tally line in, among others,
  `the_example_mapping_checks_clean`, `every_problem_in_a_mapping_is_reported_at_once`,
  `what_a_construct_writes_is_checked_against_the_stated_shape`, `a_mapping_with_no_shape_says_so`,
  `an_unchanged_corpus_reports_nothing` and `a_changed_format_is_reported_document_by_document`;
  the doctest on `Report::to_text` pins its header, tally and finding line.
- **No automated accessibility checker, browser or assistive technology has been used on this
  output.**

## Known limitations

Found by reading the source on 2026-10-31, not by testing:

- The tally calls the lowest severity a `note`, while each finding line labels it `info`: one
  severity, two words.
- Finding lines separate severity, code and message only by runs of spaces, padded to align
  columns (`  error    profile-invalid  …`); how a screen reader or braille display presents that
  has not been tried.
- One time-budget refusal in `src/sparql.rs` cites an internal work-item number in parentheses,
  which a reader outside the project cannot look up.
- The examples print `lift failed: …` or `query … failed: …` with the underlying error and say
  nothing more about what to do.

## Contributing

Read this file before changing any output listed above: a lint report, an error message or an
example's output. Keep meaning in words, keep a finding one line, and update the pinned text in
`tests/lint.rs` on purpose. Say in the pull request what was tested, with what, and what was not.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-markdown/issues/new?labels=accessibility) with
the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a maintainer
adds it): what you were doing, what happened, and your terminal, editor or browser and any
assistive technology, if you are comfortable sharing them. Anything security-sensitive goes
through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-markdown/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |
