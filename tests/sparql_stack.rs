//! Caller text that a recursive parser would overflow the stack on is refused, or run on a
//! stack big enough for it — and never aborts the host (ledger #963).
//!
//! The claim, found by the ikigai-store arc (ledger #915): oxigraph's SPARQL parser and
//! evaluator recurse, so a query nested or chained deeply enough overflows the stack of the
//! thread running it, and Rust aborts the WHOLE PROCESS on a stack overflow, on any thread.
//! Here the SPARQL is the mapping's `md:construct`, which a caller supplies by value in
//! `mapping=` (or by reference to a resource it controls). It is parsed and evaluated
//! twice: once by `Mapping::parse` (over an empty store, to check it is a CONSTRUCT) and
//! once by `apply` (over the lifted document).
//!
//! Every reproduction therefore runs in a CHILD PROCESS — this test binary re-executed with
//! one probe named in its environment — on a 2 MiB thread, the size of a tokio worker's. The
//! parent asserts on the child's exit: an abort kills the child, never this binary, and reads
//! as a failure with the signal named.

use std::process::Command;
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Verb};

const PROBE: &str = "IKIGAI_MARKDOWN_STACK_PROBE";
/// `ikigai_store::limits::MAX_SPARQL_NESTING`, restated so the reproduction compiles
/// against the release that predates the bound.
const BOUND: usize = 64;

const DOC: &str = "# Title\n\nA paragraph.\n";

/// A mapping carrying one CONSTRUCT whose WHERE is `body`.
fn mapping(construct: &str) -> String {
    format!(
        "@prefix md: <https://ikigai-rs.dev/ns/md#> .\n\
         <#m> a md:Mapping ; md:construct \"\"\"{construct}\"\"\" ."
    )
}

/// `n` levels of parentheses inside a FILTER: two levels (`{`, `FILTER(`) plus `n`.
fn parens(n: usize) -> String {
    format!(
        "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o FILTER({}true{}) }}",
        "(".repeat(n),
        ")".repeat(n)
    )
}

/// One lift request, as `(src, mapping)`, named by a probe so a child can rebuild it.
fn request(case: &str, n: usize) -> (String, Option<String>) {
    match case {
        "construct-parens" => (DOC.to_string(), Some(mapping(&parens(n)))),
        // Not nesting: a FLAT `||` chain, which the evaluator still recurses over once per
        // term. Inline on a 2 MiB thread a few hundred terms overflow a debug build.
        "construct-or-chain" => (
            DOC.to_string(),
            Some(mapping(&format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o FILTER(false{}) }}",
                "||false".repeat(n)
            ))),
        ),
        "construct-bangs" => (
            DOC.to_string(),
            Some(mapping(&format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o FILTER({}true) }}",
                "!".repeat(n)
            ))),
        ),
        // Inside the stack bounds, and costly in TIME: the planner cannot be interrupted.
        "construct-path" => (
            DOC.to_string(),
            Some(mapping(&format!(
                "PREFIX md: <https://ikigai-rs.dev/ns/md#> CONSTRUCT {{ ?s md:x ?o }} WHERE {{ ?s md:a{} ?o }}",
                "/md:a".repeat(n)
            ))),
        ),
        "construct-or1" => (
            DOC.to_string(),
            Some(mapping(&format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o FILTER(1{}) }}",
                "||1".repeat(n)
            ))),
        ),
        "construct-bgp" => (
            DOC.to_string(),
            Some(mapping(&format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ {} }}",
                (0..n).map(|i| format!("?s ?p ?o{i} .")).collect::<Vec<_>>().join(" ")
            ))),
        ),
        // The other caller text this module parses, here as evidence of which parsers
        // recurse. The mapping's Turtle (oxttl keeps its own stack):
        "mapping-bnodes" => (
            DOC.to_string(),
            Some(format!(
                "@prefix md: <https://ikigai-rs.dev/ns/md#> .\n<#m> a md:Mapping ; \
                 md:note {}<urn:o>{} .",
                "[ md:note ".repeat(n),
                " ]".repeat(n)
            )),
        ),
        // The document's YAML frontmatter, a compact nested block sequence: two bytes a
        // level, and `- - - x` is ordinary YAML. Reached through the lift's own `src`.
        "frontmatter-seq" => (format!("---\n{}x\n---\n\n# T\n", "- ".repeat(n)), None),
        // The same depth as nested flow sequences (the scanner caps those at 255 levels)
        // and as indented block mappings.
        "frontmatter-flow" => (
            format!("---\n{}x{}\n---\n\n# T\n", "[".repeat(n), "]".repeat(n)),
            None,
        ),
        "frontmatter-map" => (
            format!(
                "---\n{}x: 1\n---\n\n# T\n",
                (0..n).map(|i| format!("{}k:\n", " ".repeat(i))).collect::<String>()
                    + &" ".repeat(n)
            ),
            None,
        ),
        // The markdown itself: nested block quotes and nested lists.
        "blockquotes" => (format!("{}x\n", "> ".repeat(n)), None),
        "lists" => (format!("{}x\n", "- ".repeat(n)), None),
        // A citation regex (the `regex` crate bounds its own nesting).
        "citation-regex" => (
            DOC.to_string(),
            Some(format!(
                "@prefix md: <https://ikigai-rs.dev/ns/md#> .\n<#m> a md:Mapping ; \
                 md:citation [ md:name \"c\" ; md:pattern \"{}x{}\" ] .",
                "(".repeat(n),
                ")".repeat(n)
            )),
        ),
        other => panic!("no probe named {other}"),
    }
}

fn issue(case: &str, n: usize) -> ikigai_core::Result<String> {
    let (src, mapping) = request(case, n);
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space()));
    let mut req = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(src.into_bytes()));
    if let Some(m) = mapping {
        req = req.with_arg("mapping", ArgRef::Inline(m.into_bytes()));
    }
    block_on(kernel.issue(req, &Capability::root()))
        .map(|rep| String::from_utf8_lossy(&rep.bytes).into_owned())
}

/// The child's half: inert unless a parent named a probe. Runs it on a 2 MiB thread — a
/// tokio worker's stack — prints the outcome, and exits before the harness can.
#[test]
fn probe_child() {
    let Ok(spec) = std::env::var(PROBE) else {
        return;
    };
    let (case, n) = spec.split_once(':').unwrap();
    let (case, n) = (case.to_string(), n.parse::<usize>().unwrap());
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || match issue(&case, n) {
            Ok(text) => format!("ok {}", text.replace('\n', " ")),
            Err(e) => format!("err {}", e.to_string().replace('\n', " ")),
        })
        .unwrap()
        .join()
        .unwrap();
    println!("\nOUTCOME {outcome}");
    std::process::exit(0);
}

/// The parent's half: run one probe in a child and return what it said, or fail naming how
/// it died — an abort is the defect.
fn probe(case: &str, n: usize) -> String {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, format!("{case}:{n}"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the `{case}` probe at {n} did not survive a 2 MiB thread: {} — {}",
        out.status,
        stderr
            .lines()
            .find(|l| l.contains("overflow"))
            .unwrap_or(&stderr)
    );
    let at = stdout
        .find("\nOUTCOME ")
        .unwrap_or_else(|| panic!("the `{case}` probe reported nothing: {stdout}"));
    stdout[at + "\nOUTCOME ".len()..]
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

// ------------------------------------------------------------------ the reproduction

#[test]
fn three_thousand_parentheses_in_a_construct_are_refused_on_mapping_and_abort_nothing() {
    // On main before ledger #963 this aborted the child: `has overflowed its stack`.
    let outcome = probe("construct-parens", 3000);
    assert!(
        outcome.starts_with("err invalid argument `mapping`: md:construct: ")
            && outcome.contains(&format!("deeper than {BOUND}")),
        "{outcome}"
    );
}

#[test]
fn a_run_of_not_is_nesting_and_is_refused() {
    // One `!` is one level of the parser's recursion; also an abort before ledger #963.
    let outcome = probe("construct-bangs", 3000);
    assert!(
        outcome.starts_with("err invalid argument `mapping`")
            && outcome.contains(&format!("deeper than {BOUND}")),
        "{outcome}"
    );
}

#[test]
fn a_long_flat_chain_runs_on_the_stack_the_construct_is_given() {
    // Not nesting, so no lexical bound refuses it, and it aborted a 2 MiB thread in a debug
    // build before ledger #963. It is parsed and evaluated on a stack sized for its length.
    let outcome = probe("construct-or-chain", 600);
    assert!(outcome.starts_with("ok "), "{outcome}");
}

// ------------------------------------------------------------------ at the bound, and time

#[test]
fn a_construct_at_the_bound_parses_and_runs() {
    // `{` and `FILTER(` are two levels; the rest brings it to exactly the bound.
    let outcome = probe("construct-parens", BOUND - 2);
    assert!(outcome.starts_with("ok "), "{outcome}");
    let outcome = probe("construct-parens", BOUND - 1);
    assert!(
        outcome.contains(&format!("deeper than {BOUND}")),
        "{outcome}"
    );
}

#[test]
fn an_algebra_the_planner_would_spend_minutes_on_is_refused_before_planning() {
    // Measured through this endpoint on a release build without this bound: 66 s for the
    // 250 patterns, 70 s for the 1,000-step path, 4.8 s for the 10,000-term chain — each
    // inside the byte and nesting bounds, and each planned twice (to check the mapping,
    // then to apply it). Now each is refused in milliseconds.
    for (case, n, bound) in [
        ("construct-bgp", 250, "MAX_JOIN_OPERANDS"),
        ("construct-path", 1000, "MAX_JOIN_OPERANDS"),
        ("construct-or1", 10_000, "MAX_ALGEBRA_NODES"),
    ] {
        let outcome = probe(case, n);
        assert!(
            outcome.starts_with("err invalid argument `mapping`") && outcome.contains(bound),
            "{case}: {outcome}"
        );
    }
}

// ------------------------------------------------------------------ the public API

#[test]
fn the_public_parse_and_apply_are_bounded_too() {
    // Both refuse before anything is parsed, so this runs in-process.
    let deep = parens(3000);
    let err = ikigai_markdown::Mapping::parse(&mapping(&deep), "urn:ex:m").unwrap_err();
    assert!(err.contains(&format!("deeper than {BOUND}")), "{err}");
    let graph = oxigraph::model::NamedNode::new("urn:ex:doc").unwrap();
    let err = ikigai_markdown::apply(&[], &graph, &[deep]).unwrap_err();
    assert!(
        err.starts_with("construct 1: ") && err.contains(&format!("deeper than {BOUND}")),
        "{err}"
    );
}

// ------------------------------------------------------------------ the other parsers

#[test]
fn the_other_parsers_of_caller_text_do_not_recurse_on_nesting() {
    // The mapping's Turtle (oxttl keeps its own stack), the markdown (pulldown-cmark builds
    // a tree iteratively, and the lift walks its events with an explicit stack).
    for case in ["mapping-bnodes", "blockquotes", "lists"] {
        let outcome = probe(case, 100_000);
        assert!(outcome.starts_with("ok "), "{case}: {outcome}");
    }
    // A citation regex: the `regex` crate refuses past its own nesting limit.
    let outcome = probe("citation-regex", 3000);
    assert!(
        outcome.starts_with("err invalid argument `mapping`") && outcome.contains("not a regex"),
        "{outcome}"
    );
}

#[test]
fn deeply_nested_frontmatter_is_read_without_recursion() {
    // yaml-rust2's `Parser::load` recurses once per level of nesting, and on main
    // `- - - … x` 100,000 deep in a document's frontmatter aborted the host (ledger #963).
    // The lift now reads the parser's events in a loop and keeps its own stack.
    let outcome = probe("frontmatter-seq", 100_000);
    assert!(
        outcome.starts_with("ok ") && outcome.contains("md#Field"),
        "{outcome}"
    );
    // Flow sequences past the scanner's 255 levels are a YAML error, recorded on the
    // document as data.
    let outcome = probe("frontmatter-flow", 100_000);
    assert!(
        outcome.starts_with("ok ") && outcome.contains("frontmatterError"),
        "{outcome}"
    );
    // Indented mappings cost n²/2 bytes, so 2,000 levels is 2 MB of frontmatter.
    let outcome = probe("frontmatter-map", 2_000);
    assert!(
        outcome.starts_with("ok ") && outcome.contains("md#Field"),
        "{outcome}"
    );
}
