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
| Last reviewed | 2026-11-12 |

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

Reviewed in the source on 2026-11-12; the checks and their limits are recorded below:

- Severity is a word on every finding (`error`, `warning`, `info`), never only a color or a
  symbol, and the tally spells out its counts with singular and plural forms.
- A finding is one line: a parser's multi-line message is joined (`Finding::new`), so a reader
  moving line by line meets one finding per line.
- IRIs in finding messages are abbreviated with the mapping's own prefixes
  (`Report::abbreviate`), so a report reads in the mapping author's words.
- The report is sorted, so the same inputs give the same text.
- A mapping that does not load is a finding in the report, not a refusal.
- A refusal names the argument (`src`, `as`, `mapping`, `name`, `path`) and says what was
  expected; a missing named mapping names the IRI that was asked for, and says when no file
  mount serves it; the time-budget refusals start with what to do (narrow the construct, lint fewer
  documents, or ask the host for a larger budget).
- Nothing is written in color, by the endpoints or by the examples.

## How it is tested

Every named check has a result: Pass, Fail or Blocked, with evidence. A Pass covers only
the stated check, not overall accessibility or WCAG conformance. The Rust tests and source
reviews below were performed on 2026-11-12 against main commit `6a80b12d932913107313c9ddd8a294e71e7cc90b`, before
this documentation update. `cargo test` passed with Cargo 1.99.0 and rustc 1.99.0; it runs
the test files and doctests named below. CI repeats these Rust tests, but does not evaluate
output with assistive technology. Source review used ripgrep 15.2.0; it does not establish
how a terminal, editor, browser or assistive technology presents the output.

| Check | Result | Evidence |
|---|---|---|
| Report headers, severity words, tally singular/plural forms and document grouping | Pass | `cargo test`: `tests/lint.rs`, including `the_example_mapping_checks_clean`, `every_problem_in_a_mapping_is_reported_at_once`, `a_mapping_with_no_shape_says_so` and `a_changed_format_is_reported_document_by_document`; `Report::to_text` doctest in `src/lint.rs`. Pins sample text and tally lines, not every possible report or its spoken presentation. |
| Multi-line finding messages joined to one line | Pass | Source review of `Finding::new` in `src/lint.rs`: messages containing a newline are joined with spaces. Checks the implementation, not terminal wrapping or caller-supplied document paths. |
| Prefix abbreviation and deterministic report order | Pass | Source review of `Report::abbreviate`, `Report::sort` and their callers in `src/lint.rs` and `src/corpus.rs`; `cargo test`: `tests/lint.rs` pins abbreviated messages and document order. Covers source behavior and fixtures, not readability of every mapping's prefixes. |
| Unloadable mappings reported as findings | Pass | `cargo test`: `turtle_that_does_not_parse_is_a_finding_through_the_endpoint` and `a_mapping_that_does_not_load_is_a_finding_not_a_refusal` in `tests/lint.rs`. Covers these error paths, not all host failures. |
| Refusals name arguments and explain expected input; missing named mappings identify the requested resource and missing mount | Pass | Source review of argument refusals in `src/lib.rs`, `src/named.rs` and `src/corpus.rs`; `cargo test`: refusal cases in `tests/lint.rs` and missing-name/missing-mount cases in `tests/named.rs`. Covers local messages and fixture assertions, not every underlying host error. |
| Time-budget refusals lead with recovery advice and omit internal work-item references | Pass | `cargo test`: `time_budget_refusals_lead_with_an_action_and_omit_internal_references` in `src/sparql.rs` checks all three messages; `tests/eval_budget.rs` and `tests/lint.rs` exercise public refusals. Does not test whether people can follow the advice. |
| Output meaning independent of color | Pass | Source review of report formatting in `src/lint.rs`, endpoint messages in `src/lib.rs`, `src/named.rs`, `src/sparql.rs`, `src/corpus.rs`, and printing in `examples/lift.rs`, `examples/lint.rs`, `examples/reconcile.rs`: severity is written in words and these formatters add no color. Does not assess colors a host adds. |
| Consistent name for the lowest severity | Fail | Source review of `Severity::as_str` and `Report::tally` in `src/lint.rs`, corroborated by `a_mapping_with_no_shape_says_so` in `tests/lint.rs`: finding lines say `info`, the tally says `note`. No WCAG failure determination was made. |
| Recovery advice in example failure messages | Fail | Source review of `examples/lift.rs` and `examples/reconcile.rs`: `lift failed: …`, `lift failed for …` and `query … failed: …` add no recovery advice to the underlying error. No user evaluation or WCAG failure determination was made. |
| Automated accessibility checker | Blocked | No automated accessibility checker has been run on this output. No host-rendered page or browser checker is configured in this repository; Rust assertions do not substitute for one. |
| Browser rendering, zoom, reflow and contrast of presented output and documentation | Blocked | No browser evaluation was run for this review. This crate emits text, not HTML; presentation depends on the host, GitHub or an editor. No historical browser run is claimed as current evidence. |
| Manual keyboard navigation | Blocked | No person performed a keyboard pass for this review; no interactive host or terminal session with a human evaluator was available. Rust tests do not establish keyboard usability. |
| Screen reader | Blocked | No screen reader has been used on this output; no screen reader session was available for this review. Spoken order and padded finding columns remain unevaluated. |
| Screen magnifier | Blocked | No screen magnifier has been used on this output; no magnifier session was available for this review. |
| Speech input | Blocked | No speech-input software has been used on this output; no speech-input session was available for this review. |
| Switch access | Blocked | No switch-access software has been used on this output; no switch-access session was available for this review. |
| Braille display | Blocked | No braille display has been used on this output; no braille display was available for this review. Padded finding columns remain unevaluated. |

## Known limitations

Rechecked by reading the source on 2026-11-12; see the Fail and Blocked rows above:

- The tally calls the lowest severity a `note`, while each finding line labels it `info`: one
  severity, two words.
- Finding lines separate severity, code and message only by runs of spaces, padded to align
  columns (`  error    profile-invalid  …`); how a screen reader or braille display presents that
  has not been tried.
- The examples print `lift failed: …` or `query … failed: …` with the underlying error and say
  nothing more about what to do.

## Contributing

Read this file before changing any output listed above: a lint report, an error message or an
example's output. Keep meaning in words, keep a finding one line, and update the pinned text in
`tests/lint.rs` on purpose. In the pull request's accessibility section, give every check a
Check | Result | Evidence table row: Pass, Fail or Blocked, with the test or command, tool
and version used, or why it could not run. Keep unrun checks as Blocked, including assistive
technology nobody used. A historical run CI does not repeat is Blocked with its date, not Pass.

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
| 2026-11-07 | [Time-budget refusals](https://github.com/ikigai-rs/ikigai-markdown/pull/28) lead with the recovery action and omit internal work-item numbers. |
| 2026-11-12 | Record current checks as Pass, Fail or Blocked, with evidence and evaluation limits. |
