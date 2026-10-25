//! Every SPARQL text this module runs comes from a mapping, and a mapping is caller text:
//! `mapping=` carries it by value, or names a resource the caller may control. This is the
//! one place that text reaches a parser (ledger #963).
//!
//! oxigraph's SPARQL parser and evaluator are **recursive**, so a query's shape is a claim
//! on the stack of whatever thread runs it, and running out aborts the **whole process** —
//! every request the host is serving, not this one. Measured on `main` before this module
//! existed, on a 2 MiB thread (a tokio worker's): a `md:construct` nesting 3,000 parentheses,
//! a 3,000-long run of `!`, or a FLAT 600-term `||` chain each aborted the host through
//! `urn:markdown:lift` (`tests/sparql_stack.rs`).
//!
//! The bounds are `ikigai-store`'s, not a copy of them (ledger #976 is where they become
//! one shared home):
//!
//! 1. [`limits::check_sparql`] refuses text past `MAX_SPARQL_BYTES` or nesting deeper than
//!    `MAX_SPARQL_NESTING`, before the parser sees a byte.
//! 2. The parse AND the evaluation run on a thread sized from the text's length — the
//!    evaluator is lazy, so that includes draining the triples. [`EvaluationBudget::run`]
//!    spawns it, sized as [`limits::on_sparql_stack`] sizes one. On wasm there is no thread
//!    and only the first layer applies.
//! 3. [`budget::check_query`] refuses an algebra past `MAX_JOIN_OPERANDS` in one join or
//!    `MAX_ALGEBRA_NODES` in all, before oxigraph PLANS it. That is a bound on TIME, not
//!    stack: the planner cannot be interrupted. Measured through `urn:markdown:lift` on a
//!    release build without it, over a three-line document: 250 triple patterns took 66 s,
//!    a 1,000-step path 70 s, a 10,000-term `||` chain 4.8 s — every one inside the first
//!    two bounds, and each planned twice (once to check the mapping, once to apply it).
//!    Each is now refused in milliseconds.
//! 4. ★ **No `SERVICE`, in any build** (ledger #1085, #1083). A host whose graph enables
//!    `oxigraph/http-client` (rudof does, so `ikigai-cli` does) gets oxigraph's default HTTP
//!    service handler on every plain `SparqlEvaluator`, and a `SERVICE <http://…>` in a
//!    mapping became an outbound request no `urn:cap:net:*` gates: measured through
//!    `urn:markdown:lift` against a 127.0.0.1 stub, four lifts sent it 27 requests
//!    (`tests/service_egress.rs`). [`service::refuse_service_with`] refuses one anywhere in
//!    the algebra (`EXISTS`, `OPTIONAL`, `LATERAL`, a variable name, `SILENT`) before
//!    evaluation, worded for a mapping rather than the store (ledger #1108), and
//!    [`service::evaluator`] refuses the call itself behind it. `LOAD` needs no check
//!    here: it is an UPDATE, and this module only ever parses a QUERY, so a `LOAD` is refused
//!    as not SPARQL before anything evaluates.
//!
//! 5. ★ **A deadline on the evaluation** (ledger #1344). What the algebra bound leaves is a
//!    cross product: a 167-byte construct counting five unrelated patterns over a
//!    twenty-paragraph document had not answered after 30 s (`tests/eval_budget.rs`), and the
//!    DOCUMENT is caller text too. Every evaluation now runs under the space's
//!    [`EvaluationBudget`], `ikigai-store`'s own machinery for an evaluation run outside its
//!    doors, so the promises are the store's:
//!    - **The caller is answered at the budget** with a typed [`Error::Timeout`], never a
//!      partial graph.
//!    - **The evaluation is told to stop** through oxigraph's cancellation token, which it
//!      checks wherever it touches the dataset, and this module checks between triples.
//!    - ⚠ **A cross product does not stop**: oxigraph does not check the token inside the
//!      loops over rows already in memory, and nothing can stop a thread from outside it. So
//!      such an evaluation goes on holding a core after its caller was answered. It is
//!      counted, and while `TimeBudget::max_overdue` are still running, a lift that needs to
//!      evaluate is refused at once with a transient [`Error::Unavailable`].
//!
//!    **The budget is the caller's, and covers the whole lift**: the [`TimeBudget`]'s base,
//!    raised by a `urn:cap:store:budget:<ms>` grant in the caller's capability, the ceiling for
//!    root — chosen exactly as the store's doors choose it. Every construct of one lift, both
//!    when the mapping is checked and when it is applied, draws on that one allowance, so a
//!    mapping of many slow constructs is answered at the budget too, not at a multiple of it.
//!    [`crate::Mapping::parse`] and [`crate::apply`], which have no caller, run under a
//!    process-wide default and its base (5 s). On wasm there are no threads: the evaluation
//!    runs inline, as the store's does there, and cannot be stopped partway. But the
//!    allowance is still kept, on the kernel's injected clock (`inv.now()`), since
//!    `Instant::now` panics there: a construct or a
//!    snapshot's next document is refused once it is spent (ledger #2197). A host that
//!    injects no clock gets no deadline.
//!
//! Every refusal of the text is a typed [`Error::InvalidArgument`] on `mapping`, the argument
//! that carried it. Running out of time is a [`Error::Timeout`], and a lift turned away while
//! too many evaluations are overdue an [`Error::Unavailable`]: both are transient, and neither
//! says the mapping is wrong.

use ikigai_core::{Capability, Error, Result, Time};
use ikigai_store::{budget, limits, service, EvaluationBudget, TimeBudget};
use oxigraph::model::Triple;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

/// The argument every mapping's SPARQL arrives in.
pub(crate) const ARG: &str = "mapping";

/// What a mapping author does instead of a `SERVICE`, after the ecosystem's shared first
/// sentence ([`service::SERVICE_REFUSAL`]). Not the store's own remedy, which tells the caller
/// to sink into `urn:iki:store:load` — wrong advice inside a mapping (ledger #1108).
const SERVICE_REMEDY: &str = "A mapping's constructs read only the document being lifted: \
     fetch remote data through the kernel, where the network capability applies, and pass it \
     in as a document of its own.";

/// A refusal on [`ARG`].
pub(crate) fn refuse(detail: impl Into<String>) -> Error {
    Error::InvalidArgument {
        name: ARG.to_string(),
        detail: detail.into(),
    }
}

/// Where an [`Allowance`] reads the time (ledger #2197).
#[derive(Clone, Copy)]
pub(crate) enum Clock<'a> {
    /// This process's monotonic clock. Native only: on wasm `Instant::now` panics.
    #[cfg(not(target_family = "wasm"))]
    Monotonic,
    /// The invocation's, `inv.now()`: the kernel's injected [`ikigai_core::Clock`], which is
    /// the only clock a wasm host has (a browser's is `Date.now()`-backed). When it answers
    /// `None`, nothing is timed. Native builds construct it only in tests.
    #[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
    Invocation(&'a (dyn Fn() -> Option<Time> + Sync + 'a)),
    /// None: nothing is timed.
    #[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
    Untimed,
}

impl<'a> Clock<'a> {
    /// The clock of a lift with no invocation behind it ([`crate::Mapping::parse`],
    /// [`crate::apply`]): the monotonic one natively, and on wasm none.
    pub(crate) fn host() -> Clock<'a> {
        #[cfg(target_family = "wasm")]
        {
            Clock::Untimed
        }
        #[cfg(not(target_family = "wasm"))]
        {
            Clock::Monotonic
        }
    }

    /// The clock of an invocation, `now` being its `inv.now()`: natively the monotonic one,
    /// as the store's doors time an evaluation, and on wasm the kernel's.
    pub(crate) fn of(now: &'a (dyn Fn() -> Option<Time> + Sync + 'a)) -> Clock<'a> {
        #[cfg(target_family = "wasm")]
        {
            Clock::Invocation(now)
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = now;
            Clock::Monotonic
        }
    }
}

/// When an [`Allowance`] runs out, on its [`Clock`].
enum Until<'a> {
    #[cfg(not(target_family = "wasm"))]
    Instant(std::time::Instant),
    Kernel {
        now: &'a (dyn Fn() -> Option<Time> + Sync + 'a),
        at: u64,
    },
    Never,
}

/// The time ONE lift's constructs may take, all of them together: the caller's budget, and
/// when it started being spent. Every [`construct`] of the lift draws on it.
pub(crate) struct Allowance<'a> {
    budget: &'a EvaluationBudget,
    capability: &'a Capability,
    /// The caller's whole budget, for the refusal's wording.
    total: std::time::Duration,
    /// When it runs out, read on the allowance's [`Clock`].
    until: Until<'a>,
}

impl<'a> Allowance<'a> {
    /// The allowance of a caller holding `capability`, starting now on `clock`: what `budget`
    /// gives that capability, as the store's doors choose it.
    pub(crate) fn new(
        budget: &'a EvaluationBudget,
        capability: &'a Capability,
        clock: Clock<'a>,
    ) -> Allowance<'a> {
        let total = budget.time_budget().for_capability(capability);
        let until = match clock {
            #[cfg(not(target_family = "wasm"))]
            Clock::Monotonic => Until::Instant(std::time::Instant::now() + total),
            Clock::Invocation(now) => match now() {
                Some(t) => Until::Kernel {
                    now,
                    at: t
                        .as_millis()
                        .saturating_add(u64::try_from(total.as_millis()).unwrap_or(u64::MAX)),
                },
                None => Until::Never,
            },
            Clock::Untimed => Until::Never,
        };
        Allowance {
            budget,
            capability,
            total,
            until,
        }
    }

    /// What is left, in whole milliseconds, or `None` when nothing is timed (no clock, or
    /// the kernel's stopped answering).
    fn left(&self) -> Option<u128> {
        match &self.until {
            #[cfg(not(target_family = "wasm"))]
            Until::Instant(until) => Some(
                until
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis(),
            ),
            Until::Kernel { now, at } => {
                now().map(|t| u128::from(at.saturating_sub(t.as_millis())))
            }
            Until::Never => None,
        }
    }

    /// What is left, in whole milliseconds, as the `requested` bound for
    /// [`EvaluationBudget::run`], which can only lower what the capability gets. A timeout
    /// when nothing is.
    fn remaining(&self, what: &str) -> Result<Option<String>> {
        match self.left() {
            None => Ok(None),
            Some(0) => Err(Error::Timeout(format!(
                "{what} was not started: this lift's constructs had already used its whole \
                 time budget of {} ms (ledger #1344). The budget is set by the host. Narrow \
                 the constructs, or ask the host for a larger budget",
                self.total.as_millis()
            ))),
            Some(left) => Ok(Some(left.to_string())),
        }
    }

    /// Refuse with a typed timeout when nothing of the allowance is left: what a loop over
    /// many lifts checks between them. Never refuses when nothing is timed.
    pub(crate) fn check_left(&self, what: &str) -> Result<()> {
        if self.left() == Some(0) {
            return Err(Error::Timeout(format!(
                "{what} ran past the caller's time budget of {} ms and was stopped. The budget \
                 is set by the host, and a `urn:cap:store:budget:<ms>` grant raises it. Lint \
                 fewer documents at a time, or ask the host for a larger budget",
                self.total.as_millis()
            )));
        }
        Ok(())
    }

    /// A timeout in this lift's words: the store's names a `budget=` argument the lift does
    /// not take, and the milliseconds that were LEFT rather than the caller's budget.
    fn explain(&self, e: Error, what: &str, left: Option<&str>) -> Error {
        match e {
            Error::Timeout(_) => Error::Timeout(format!(
                "{what} ran past this lift's time budget of {} ms and was stopped (ledger \
                 #1344); a lift's constructs share one budget, and {} ms of it was left for this \
                 one. The budget is set by the host, and a `urn:cap:store:budget:<ms>` grant \
                 raises it. Narrow the construct — fewer unrelated patterns, a bound subject — \
                 or ask the host for a larger budget",
                self.total.as_millis(),
                left.unwrap_or("all"),
            )),
            other => other,
        }
    }
}

/// The budget [`crate::Mapping::parse`] and [`crate::apply`] run under: they have no caller,
/// so no capability to raise it, and one shared instance is what caps the cores their overdue
/// evaluations can hold.
pub(crate) fn uncalled() -> &'static EvaluationBudget {
    static BUDGET: std::sync::OnceLock<EvaluationBudget> = std::sync::OnceLock::new();
    BUDGET.get_or_init(|| EvaluationBudget::new(TimeBudget::default()))
}

/// No grants: what the public functions evaluate as, so they get the budget's base.
pub(crate) fn no_grants() -> Capability {
    Capability::scoped(std::iter::empty::<String>())
}

/// Parse `text` as a CONSTRUCT and evaluate it over `store`'s default graph, returning its
/// triples — or `None` when it parses but is not a CONSTRUCT. `what` names the query in
/// every refusal (`md:construct`, `construct 2`). It runs on what is left of `allowance`.
pub(crate) fn construct(
    text: &str,
    store: &Store,
    what: &str,
    allowance: &Allowance<'_>,
) -> Result<Option<Vec<Triple>>> {
    let bounded = |e: Error| match e {
        Error::InvalidArgument { detail, .. } => refuse(format!("{what}: {detail}")),
        other => other,
    };
    limits::check_sparql(text, ARG).map_err(bounded)?;
    let requested = allowance.remaining(what)?;
    // The evaluation may outlive this call (a cross product does not stop when told to), so
    // it owns what it reads. A clone of an in-memory `Store` shares its data.
    let (owned, store, named) = (text.to_string(), store.clone(), what.to_string());
    let work = move |token: &oxigraph::sparql::CancellationToken| {
        let bounded = |e: Error| match e {
            Error::InvalidArgument { detail, .. } => refuse(format!("{named}: {detail}")),
            other => other,
        };
        let query = spargebra::SparqlParser::new()
            .parse_query(&owned)
            .map_err(|e| refuse(format!("{named} is not valid SPARQL: {e}")))?;
        budget::check_query(&query, ARG).map_err(bounded)?;
        service::refuse_service_with(&query, ARG, Some(SERVICE_REMEDY)).map_err(bounded)?;
        let results = service::evaluator()
            .with_cancellation_token(token.clone())
            .for_query(query)
            .on_store(&store)
            .execute()
            .map_err(|e| refuse(format!("{named} does not evaluate: {e}")))?;
        let QueryResults::Graph(triples) = results else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for t in triples {
            // The caller has been answered: whatever this would return is discarded.
            if token.is_cancelled() {
                return Ok(None);
            }
            out.push(t.map_err(|e| refuse(format!("{named}: {e}")))?);
        }
        Ok(Some(out))
    };
    allowance
        .budget
        .run(text, allowance.capability, requested.as_deref(), work)
        .map_err(|e| allowance.explain(e, what, requested.as_deref()))
}

/// Parse `text` and put it through every bound and refusal [`construct`] applies BEFORE it
/// evaluates anything: the text bound, the parse, the algebra bound, and `SERVICE`. What the
/// linter reads a construct with, so it says what the lift would say without running it.
///
/// ⚠ The parser is recursive: call this inside [`limits::on_sparql_stack`], never on a
/// worker's own stack. The text bound is checked first all the same, so a caller that
/// forgets is refused before the parse on anything the bound refuses.
pub(crate) fn parse_static(text: &str, what: &str) -> Result<spargebra::Query> {
    let bounded = |e: Error| match e {
        Error::InvalidArgument { detail, .. } => refuse(format!("{what}: {detail}")),
        other => other,
    };
    limits::check_sparql(text, ARG).map_err(bounded)?;
    let query = spargebra::SparqlParser::new()
        .parse_query(text)
        .map_err(|e| refuse(format!("{what} is not valid SPARQL: {e}")))?;
    budget::check_query(&query, ARG).map_err(bounded)?;
    service::refuse_service_with(&query, ARG, Some(SERVICE_REMEDY)).map_err(bounded)?;
    Ok(query)
}

/// The detail of a refusal, for the public API that predates typed errors and returns
/// `String`; anything else as it displays.
pub(crate) fn detail(e: Error) -> String {
    match e {
        Error::InvalidArgument { detail, .. } => detail,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A construct a real mapping would carry.
    const ORDINARY: &str = "CONSTRUCT { ?s <urn:ex:seen> true } WHERE { ?s ?p ?o }";

    #[test]
    fn a_construct_draws_on_what_the_lift_has_left() {
        let budget = EvaluationBudget::new(TimeBudget::new(std::time::Duration::from_millis(50)));
        let caller = no_grants();
        let store = Store::new().unwrap();

        // Fresh, the allowance runs a construct.
        let allowance = Allowance::new(&budget, &caller, Clock::host());
        assert!(construct(ORDINARY, &store, "construct 1", &allowance)
            .unwrap()
            .is_some());

        // Spent by the constructs before it, the next is refused before it starts, however
        // cheap it is: the budget covers the lift, not each construct.
        std::thread::sleep(std::time::Duration::from_millis(80));
        let err = construct(ORDINARY, &store, "construct 2", &allowance).unwrap_err();
        let Error::Timeout(detail) = err else {
            panic!("{err}");
        };
        assert!(
            detail.starts_with("construct 2 was not started") && detail.contains("50 ms"),
            "{detail}"
        );
        assert_eq!(budget.overdue_evaluations(), 0);
    }

    #[test]
    fn the_budget_follows_the_callers_capability() {
        let budget = EvaluationBudget::new(TimeBudget::new(std::time::Duration::from_millis(50)));
        let granted = Capability::scoped([budget::cap_budget(2_000)]);
        let root = Capability::root();
        assert_eq!(
            Allowance::new(&budget, &no_grants(), Clock::host())
                .total
                .as_millis(),
            50
        );
        assert_eq!(
            Allowance::new(&budget, &granted, Clock::host())
                .total
                .as_millis(),
            2_000
        );
        assert_eq!(
            Allowance::new(&budget, &root, Clock::host()).total,
            budget.time_budget().ceiling()
        );
    }

    /// The kernel's clock, `inv.now()`, as a wasm lift reads it: a time a test moves.
    struct Kernel(std::sync::atomic::AtomicU64);
    impl Kernel {
        fn now(&self) -> Option<Time> {
            Some(Time::from_millis(
                self.0.load(std::sync::atomic::Ordering::SeqCst),
            ))
        }
        fn set(&self, millis: u64) {
            self.0.store(millis, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn the_kernels_clock_runs_an_allowance_out() {
        // What a wasm lift runs on (ledger #2197): before, an allowance there read no clock
        // and never refused.
        let budget = EvaluationBudget::new(TimeBudget::new(std::time::Duration::from_millis(50)));
        let caller = no_grants();
        let store = Store::new().unwrap();
        let kernel = Kernel(std::sync::atomic::AtomicU64::new(1_000));
        let now = || kernel.now();
        let allowance = Allowance::new(&budget, &caller, Clock::Invocation(&now));

        kernel.set(1_049);
        allowance.check_left("the snapshot").unwrap();
        assert!(construct(ORDINARY, &store, "construct 1", &allowance)
            .unwrap()
            .is_some());

        kernel.set(1_050);
        let Err(Error::Timeout(detail)) = allowance.check_left("the snapshot") else {
            panic!("an allowance on the kernel's clock did not run out");
        };
        assert!(
            detail.starts_with("the snapshot ran past") && detail.contains("50 ms"),
            "{detail}"
        );
        let Err(Error::Timeout(detail)) = construct(ORDINARY, &store, "construct 2", &allowance)
        else {
            panic!("a construct past the kernel's deadline was started");
        };
        assert!(
            detail.starts_with("construct 2 was not started"),
            "{detail}"
        );
    }

    #[test]
    fn with_no_clock_nothing_is_timed() {
        // A kernel with no clock gives an invocation no time: the honest answer is no
        // deadline, not a refusal.
        let budget = EvaluationBudget::new(TimeBudget::new(std::time::Duration::from_millis(1)));
        let caller = no_grants();
        let none = || None;
        for clock in [Clock::Invocation(&none), Clock::Untimed] {
            let allowance = Allowance::new(&budget, &caller, clock);
            std::thread::sleep(std::time::Duration::from_millis(5));
            allowance.check_left("the snapshot").unwrap();
            assert_eq!(allowance.remaining("construct").unwrap(), None);
        }
    }
}
