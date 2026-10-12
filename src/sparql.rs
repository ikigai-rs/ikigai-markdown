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
//!    process-wide default and its base (5 s). On wasm there are no threads, and so no
//!    deadline either: the evaluation runs inline, as the store's does there.
//!
//! Every refusal of the text is a typed [`Error::InvalidArgument`] on `mapping`, the argument
//! that carried it. Running out of time is a [`Error::Timeout`], and a lift turned away while
//! too many evaluations are overdue an [`Error::Unavailable`]: both are transient, and neither
//! says the mapping is wrong.

use ikigai_core::{Capability, Error, Result};
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

/// The time ONE lift's constructs may take, all of them together: the caller's budget, and
/// when it started being spent. Every [`construct`] of the lift draws on it.
pub(crate) struct Allowance<'a> {
    budget: &'a EvaluationBudget,
    capability: &'a Capability,
    /// The caller's whole budget, for the refusal's wording.
    total: std::time::Duration,
    /// When it runs out. Not on wasm, where `Instant::now` panics and nothing is timed.
    #[cfg(not(target_family = "wasm"))]
    until: std::time::Instant,
}

impl<'a> Allowance<'a> {
    /// The allowance of a caller holding `capability`, starting now: what `budget` gives that
    /// capability, as the store's doors choose it.
    pub(crate) fn new(budget: &'a EvaluationBudget, capability: &'a Capability) -> Allowance<'a> {
        let total = budget.time_budget().for_capability(capability);
        Allowance {
            budget,
            capability,
            total,
            #[cfg(not(target_family = "wasm"))]
            until: std::time::Instant::now() + total,
        }
    }

    /// What is left, in whole milliseconds, as the `requested` bound for
    /// [`EvaluationBudget::run`], which can only lower what the capability gets. A timeout
    /// when nothing is.
    fn remaining(&self, what: &str) -> Result<Option<String>> {
        #[cfg(target_family = "wasm")]
        {
            let _ = what;
            Ok(None)
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let left = self
                .until
                .saturating_duration_since(std::time::Instant::now())
                .as_millis();
            if left == 0 {
                return Err(Error::Timeout(format!(
                    "{what} was not started: this lift's constructs had already used its whole \
                     time budget of {} ms (ledger #1344). The budget is set by the host. Narrow \
                     the constructs, or ask the host for a larger budget",
                    self.total.as_millis()
                )));
            }
            Ok(Some(left.to_string()))
        }
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
        let allowance = Allowance::new(&budget, &caller);
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
        assert_eq!(Allowance::new(&budget, &no_grants()).total.as_millis(), 50);
        assert_eq!(Allowance::new(&budget, &granted).total.as_millis(), 2_000);
        assert_eq!(
            Allowance::new(&budget, &root).total,
            budget.time_budget().ceiling()
        );
    }
}
