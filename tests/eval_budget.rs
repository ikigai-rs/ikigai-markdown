//! A lift answers within a time budget, whatever its mapping's constructs do (ledger #1344).
//!
//! The claim: a mapping's `md:construct` is caller text (`mapping=` by value, or a resource the
//! caller controls), and so is the document it runs over. The algebra bounds (ledger #963) stop
//! a construct from planning for minutes, but nothing stopped one from EVALUATING for minutes:
//! a cross product of unrelated patterns over an ordinary document ran on with the caller
//! waiting. The lift now runs its constructs under `ikigai_store::EvaluationBudget`.
//!
//! Every probe runs in a CHILD PROCESS — this test binary re-executed with the probe named in
//! its environment — because a cross product does not stop when its caller is answered: oxigraph
//! checks its cancellation token only where it touches the dataset, never inside the loops over
//! rows already in memory. The child exits once it has printed, which ends that work; the parent
//! kills a child that has not answered within [`PATIENCE`] and fails, naming it.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, EndpointSpace, Error, Iri, Kernel, Request, Verb};
use ikigai_markdown::{EvaluationBudget, MappingHome, TimeBudget};

const PROBE: &str = "IKIGAI_MARKDOWN_BUDGET_PROBE";

/// How long the parent waits for a child before calling it hung: well past the default budget
/// (5 s), and far short of what the cross product below takes.
const PATIENCE: Duration = Duration::from_secs(30);

/// An ordinary document: twenty paragraphs under one heading.
fn document() -> String {
    let mut doc = String::from("# Notes\n\n");
    for i in 0..20 {
        doc.push_str(&format!("Paragraph {i} says something ordinary.\n\n"));
    }
    doc
}

/// A mapping carrying one CONSTRUCT per entry of `constructs`.
fn mapping(constructs: &[&str]) -> String {
    let mut ttl = String::from("@prefix md: <https://ikigai-rs.dev/ns/md#> .\n<#m> a md:Mapping");
    for c in constructs {
        ttl.push_str(&format!(" ;\n    md:construct \"\"\"{c}\"\"\""));
    }
    ttl.push_str(" .\n");
    ttl
}

/// 167 bytes, inside every algebra bound: a count of five unrelated patterns. Over the twenty
/// paragraphs that is on the order of 10^10 rows, counted in an aggregate, which consumes its
/// whole input before answering and so never checks oxigraph's cancellation token. Counting
/// rather than emitting keeps the child's memory flat while it runs.
const CROSS: &str =
    "CONSTRUCT { <urn:ex:doc> <urn:ex:rows> ?n } WHERE { { SELECT (COUNT(*) AS ?n) \
     WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l . ?m ?o ?p } } }";

/// A construct a real mapping would carry: one row per paragraph.
const ORDINARY: &str = "PREFIX md: <https://ikigai-rs.dev/ns/md#> \
     CONSTRUCT { ?p <urn:ex:seen> true } WHERE { ?p a md:Paragraph }";

/// A caller holding no budget grant: it gets the base budget.
fn ordinary_caller() -> Capability {
    Capability::scoped(["urn:cap:example:read"])
}

/// A space whose lifts run under `base` for an ordinary caller, at most `max_overdue` of
/// them still running after their callers were answered.
fn space(base: Duration, max_overdue: usize) -> EndpointSpace {
    let budget = EvaluationBudget::new(TimeBudget::new(base).with_max_overdue(max_overdue));
    ikigai_markdown::space_with_budget(Arc::new(MappingHome::new(None)), budget)
}

fn lift(space: EndpointSpace, constructs: &[&str], capability: &Capability) -> String {
    let kernel = Kernel::new(Arc::new(space));
    let req = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(document().into_bytes()))
        .with_arg("mapping", ArgRef::Inline(mapping(constructs).into_bytes()));
    outcome(block_on(kernel.issue(req, capability)).map(|rep| rep.bytes))
}

/// One line saying how a lift ended: the error's kind first, so a probe can assert on it.
fn outcome(result: ikigai_core::Result<Vec<u8>>) -> String {
    match result {
        Ok(bytes) => format!("ok {}", String::from_utf8_lossy(&bytes).replace('\n', " ")),
        Err(e @ Error::Timeout(_)) => format!("timeout {}", flat(&e)),
        Err(e @ Error::Unavailable(_)) => format!("unavailable {}", flat(&e)),
        Err(e) => format!("err {}", flat(&e)),
    }
}

fn flat(e: &Error) -> String {
    e.to_string().replace('\n', " ")
}

/// Run one probe and say how each of its lifts ended and how long it took, one per line.
fn run_probe(case: &str) -> Vec<String> {
    let timed = |f: &dyn Fn() -> String| {
        let start = Instant::now();
        let outcome = f();
        format!("{} {outcome}", start.elapsed().as_millis())
    };
    match case {
        // What a host mounts by default, and an ordinary caller.
        "default" => vec![timed(&|| {
            lift(ikigai_markdown::space(), &[CROSS], &ordinary_caller())
        })],
        // A host that sets a short budget.
        "short" => vec![timed(&|| {
            lift(
                space(Duration::from_millis(300), 8),
                &[CROSS],
                &ordinary_caller(),
            )
        })],
        // A grant raises the budget, as at the store's doors.
        "grant" => vec![timed(&|| {
            let caller = Capability::scoped([
                "urn:cap:example:read".to_string(),
                ikigai_store::budget::cap_budget(1500),
            ]);
            lift(space(Duration::from_millis(300), 8), &[CROSS], &caller)
        })],
        // The cap: one overdue evaluation, and the next lift is turned away at once. One space,
        // so both lifts share the count, as every lift through a host's space does.
        "overdue" => {
            let shared = space(Duration::from_millis(300), 1);
            let kernel = Arc::new(Kernel::new(Arc::new(shared)));
            let one = |constructs: &[&str]| {
                let req =
                    Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
                        .with_arg("src", ArgRef::Inline(document().into_bytes()))
                        .with_arg("mapping", ArgRef::Inline(mapping(constructs).into_bytes()));
                outcome(block_on(kernel.issue(req, &ordinary_caller())).map(|rep| rep.bytes))
            };
            vec![
                timed(&|| one(&[CROSS])),
                timed(&|| one(&[ORDINARY])),
                timed(&|| one(&[])),
            ]
        }
        // An ordinary mapping is far inside even a short budget.
        "ordinary" => vec![timed(&|| {
            lift(
                space(Duration::from_millis(300), 8),
                &[ORDINARY],
                &ordinary_caller(),
            )
        })],
        // The public functions have no caller: the default base, 5 s.
        "public" => vec![timed(&|| {
            let quads = ikigai_markdown::lift(
                &document(),
                &ikigai_markdown::DocRef {
                    iri: oxigraph::model::NamedNode::new("urn:ex:doc").unwrap(),
                    path: "notes.md".into(),
                    source: None,
                },
                &Default::default(),
            );
            let graph = oxigraph::model::NamedNode::new("urn:ex:doc").unwrap();
            match ikigai_markdown::apply(&quads, &graph, &[CROSS.to_string()]) {
                Ok(q) => format!("ok {} quads", q.len()),
                Err(e) => format!("err {}", e.replace('\n', " ")),
            }
        })],
        other => panic!("no probe named {other}"),
    }
}

/// The child's half: inert unless a parent named a probe. Prints each outcome and exits, which
/// ends any evaluation still running after its caller was answered.
#[test]
fn probe_child() {
    let Ok(case) = std::env::var(PROBE) else {
        return;
    };
    for line in run_probe(&case) {
        println!("\nOUTCOME {line}");
    }
    std::process::exit(0);
}

/// The parent's half: run one probe in a child and return its outcomes as `(elapsed, text)`, or
/// fail if it has not answered within [`PATIENCE`].
fn probe(case: &str) -> Vec<(Duration, String)> {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, case)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > PATIENCE {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "the `{case}` probe had not answered after {} s: the lift is not bounded in time",
                PATIENCE.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the `{case}` probe failed: {} — {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let outcomes: Vec<(Duration, String)> = stdout
        .split("\nOUTCOME ")
        .skip(1)
        .map(|chunk| {
            let line = chunk.lines().next().unwrap_or("");
            let (ms, text) = line.split_once(' ').unwrap();
            (Duration::from_millis(ms.parse().unwrap()), text.to_string())
        })
        .collect();
    assert!(
        !outcomes.is_empty(),
        "the `{case}` probe reported nothing: {stdout}"
    );
    outcomes
}

// ------------------------------------------------------------------ the reproduction

#[test]
fn a_cross_product_is_answered_at_the_default_budget() {
    // On main before ledger #1344 this child was still running when killed at 30 s.
    let outcomes = probe("default");
    let (elapsed, text) = &outcomes[0];
    assert!(text.starts_with("timeout "), "{text}");
    assert!(text.contains("time budget of 5000 ms"), "{text}");
    assert!(*elapsed < Duration::from_secs(10), "{elapsed:?}: {text}");
}

// ------------------------------------------------------------------ who sets the budget

#[test]
fn a_host_budget_answers_the_caller_at_it() {
    let (elapsed, text) = &probe("short")[0];
    assert!(text.starts_with("timeout "), "{text}");
    assert!(text.contains("time budget of 300 ms"), "{text}");
    assert!(*elapsed < Duration::from_secs(3), "{elapsed:?}: {text}");
}

#[test]
fn a_budget_grant_raises_it_as_at_the_store_doors() {
    let (elapsed, text) = &probe("grant")[0];
    assert!(text.starts_with("timeout "), "{text}");
    assert!(text.contains("time budget of 1500 ms"), "{text}");
    assert!(
        *elapsed >= Duration::from_millis(1500),
        "{elapsed:?}: {text}"
    );
    assert!(*elapsed < Duration::from_secs(5), "{elapsed:?}: {text}");
}

// ------------------------------------------------------------------ what cannot be stopped

#[test]
fn evaluations_still_running_past_their_budget_are_capped() {
    let outcomes = probe("overdue");
    let (_, first) = &outcomes[0];
    assert!(first.starts_with("timeout "), "{first}");
    // The cross product is still running: a lift that needs SPARQL is turned away at once,
    // transiently, rather than holding another core.
    let (elapsed, second) = &outcomes[1];
    assert!(second.starts_with("unavailable "), "{second}");
    assert!(
        *elapsed < Duration::from_millis(250),
        "{elapsed:?}: {second}"
    );
    // A lift with no construct evaluates nothing, so it is never turned away.
    let (_, third) = &outcomes[2];
    assert!(third.starts_with("ok "), "{third}");
}

// ------------------------------------------------------------------ and nothing else changes

#[test]
fn an_ordinary_mapping_is_far_inside_a_short_budget() {
    let (_, text) = &probe("ordinary")[0];
    assert!(
        text.starts_with("ok ") && text.contains("<urn:ex:seen>"),
        "{text}"
    );
}

#[test]
fn the_public_apply_runs_under_the_default_base() {
    let (elapsed, text) = &probe("public")[0];
    assert!(
        text.starts_with("err timeout: construct 1 ran past") && text.contains("of 5000 ms"),
        "{text}"
    );
    assert!(*elapsed < Duration::from_secs(10), "{elapsed:?}: {text}");
}
