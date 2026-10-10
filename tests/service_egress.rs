//! A mapping's SPARQL never reaches the network, in any build (ledger #1085, #1083).
//!
//! The claim: a mapping's `md:construct` is caller text (`mapping=` carries it by value), and
//! when any crate in a host's graph enables `oxigraph/http-client` (rudof_rdf does, on every
//! native target, so `ikigai-cli` has it), oxigraph's `SparqlEvaluator` installs a default HTTP
//! service handler: `SERVICE <http://…>` inside a construct becomes an outbound request with no
//! `urn:cap:net:*` anywhere near it. This crate cannot turn the feature off for itself.
//!
//! ★ Reproduced with this crate's own `http-client` feature (`--features http-client`; CI runs
//! a job with it), which enables exactly `oxigraph/http-client`, the switch those hosts get by
//! unification. The CONTROL (raw oxigraph reaches the stub) is cfg'd on it, the sound
//! direction: the feature on implies the handler is installed. The REFUSALS are not cfg'd on
//! anything, because a host reaching the feature through rudof never enables this crate's.
//!
//! `LOAD` is an UPDATE, and this crate only ever parses a mapping's text as a QUERY, so a
//! `LOAD` is refused as not SPARQL before anything evaluates; it is pinned here anyway.
//!
//! Every request goes to a stub on 127.0.0.1 with an ephemeral port, never a real host.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Error, Iri, Kernel, Request, Verb};
use oxigraph::model::{GraphName, NamedNode, Quad};

/// A plain-HTTP stub that records the request line of every connection it is sent, and
/// answers each with an empty SPARQL result set (so a client that does reach it finishes).
struct Stub {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

/// The first bytes of the connection [`Stub::requests`] makes itself. Accepts are served in
/// backlog order, so once the stub has answered this one, every connection made before it has
/// been recorded: the negative assertions need no sleep and cannot race.
const SENTINEL: &[u8] = b"SENTINEL\r\n";

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let head = read_head(&mut stream);
                if head.as_bytes().starts_with(SENTINEL) {
                    let _ = stream.write_all(b"ok");
                    continue;
                }
                record
                    .lock()
                    .unwrap()
                    .push(head.lines().next().unwrap_or("").to_string());
                let body = r#"{"head":{"vars":["s","p","o"]},"results":{"bindings":[]}}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/sparql-results+json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Stub { addr, seen }
    }

    fn url(&self) -> String {
        format!("http://{}/sparql", self.addr)
    }

    /// Every request line the stub has been sent, after a sentinel round trip.
    fn requests(&self) -> Vec<String> {
        let mut sentinel = TcpStream::connect(self.addr).unwrap();
        sentinel.write_all(SENTINEL).unwrap();
        let mut ack = Vec::new();
        sentinel.read_to_end(&mut ack).unwrap();
        assert_eq!(ack, b"ok", "the stub did not answer its sentinel");
        self.seen.lock().unwrap().clone()
    }
}

/// Read up to the end of the request headers (or the sentinel line), enough to log it.
fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
        head.push(byte[0]);
        if head == SENTINEL || head.len() > 64 * 1024 {
            break;
        }
        if head.ends_with(b"\r\n\r\n") {
            // Drain a body too, or closing with it unread resets the client's connection.
            let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
            let length = text
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|n| n.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            break;
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

const DOC: &str = "# Title\n\nA paragraph.\n";

/// A mapping carrying one construct.
fn mapping(construct: &str) -> String {
    format!(
        "@prefix md: <https://ikigai-rs.dev/ns/md#> .\n\
         <#m> a md:Mapping ; md:construct \"\"\"{construct}\"\"\" ."
    )
}

/// Every construct shape that names a service at `url`, labeled.
fn service_constructs(url: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "constant",
            format!("CONSTRUCT {{ ?s ?p ?o }} WHERE {{ SERVICE <{url}> {{ ?s ?p ?o }} }}"),
        ),
        (
            "silent",
            format!("CONSTRUCT {{ ?s ?p ?o }} WHERE {{ SERVICE SILENT <{url}> {{ ?s ?p ?o }} }}"),
        ),
        // One level down, where only a walk of the whole algebra sees it.
        (
            "nested",
            format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o FILTER EXISTS {{ OPTIONAL {{ SERVICE <{url}> {{ ?a ?b ?c }} }} }} }}"
            ),
        ),
        // A variable service name: the IRI arrives as a binding, so no textual check sees it.
        (
            "variable",
            format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ BIND(<{url}> AS ?endpoint) LATERAL {{ SERVICE ?endpoint {{ ?s ?p ?o }} }} }}"
            ),
        ),
    ]
}

fn lift(mapping: &str) -> ikigai_core::Result<String> {
    let kernel = Kernel::new(Arc::new(ikigai_markdown::space()));
    let req = Request::new(Verb::Source, Iri::parse(ikigai_markdown::LIFT_IRI).unwrap())
        .with_arg("src", ArgRef::Inline(DOC.as_bytes().to_vec()))
        .with_arg("mapping", ArgRef::Inline(mapping.as_bytes().to_vec()));
    block_on(kernel.issue(req, &Capability::root()))
        .map(|rep| String::from_utf8_lossy(&rep.bytes).into_owned())
}

/// The refusal's text, as `ikigai_store::service` words it.
const REFUSED: &str = "`SERVICE` is not available";

#[test]
fn a_service_in_a_mapping_is_refused_on_mapping_and_fetches_nothing() {
    let stub = Stub::start();
    let mut wrong = Vec::new();
    for (label, construct) in service_constructs(&stub.url()) {
        match lift(&mapping(&construct)) {
            Err(Error::InvalidArgument { name, detail })
                if name == "mapping"
                    && detail.contains(REFUSED)
                    && detail.contains("md:construct") => {}
            other => wrong.push(format!("{label}: {:.200}", format!("{other:?}"))),
        }
    }
    // Both halves in one report: the answers, and what the stub was sent meanwhile.
    let sent = stub.requests();
    assert!(
        wrong.is_empty() && sent.is_empty(),
        "not refused on `mapping`: {wrong:#?}\nrequests the stub was sent: {sent:#?}"
    );
}

#[test]
fn the_public_parse_and_apply_refuse_a_service_too() {
    let stub = Stub::start();
    let graph = NamedNode::new("urn:test:doc").unwrap();
    let structural = vec![Quad::new(
        NamedNode::new("urn:s").unwrap(),
        NamedNode::new("urn:p").unwrap(),
        NamedNode::new("urn:o").unwrap(),
        GraphName::DefaultGraph,
    )];
    for (label, construct) in service_constructs(&stub.url()) {
        let parsed = ikigai_markdown::Mapping::parse(&mapping(&construct), "urn:test:mapping");
        assert!(
            matches!(&parsed, Err(detail) if detail.contains(REFUSED)),
            "{label}: Mapping::parse did not refuse: {:?}",
            parsed.map(|m| m.constructs)
        );
        // `apply` takes constructs unchecked by `parse`, so it is a door of its own.
        let applied = ikigai_markdown::apply(&structural, &graph, &[construct]);
        assert!(
            matches!(&applied, Err(detail) if detail.contains(REFUSED)),
            "{label}: apply did not refuse: {applied:?}"
        );
    }
    assert_eq!(
        stub.requests(),
        Vec::<String>::new(),
        "parse or apply reached the network"
    );
}

#[test]
fn a_load_in_a_mapping_is_not_a_query_and_fetches_nothing() {
    let stub = Stub::start();
    let load = format!("LOAD <http://{}/doc.ttl>", stub.addr);
    match lift(&mapping(&load)) {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "mapping"),
        other => panic!("a LOAD in md:construct was not refused: {other:?}"),
    }
    let applied = ikigai_markdown::apply(&[], &NamedNode::new("urn:g").unwrap(), &[load]);
    assert!(applied.is_err(), "apply ran a LOAD: {applied:?}");
    assert_eq!(stub.requests(), Vec::<String>::new(), "a LOAD was fetched");
}

/// The control: in this build the hole is real. Raw oxigraph, with the feature on, sends the
/// stub a request, so the refusals above are not vacuous.
#[cfg(feature = "http-client")]
#[test]
fn control_raw_oxigraph_reaches_the_stub_with_the_feature_on() {
    use oxigraph::sparql::{QueryResults, SparqlEvaluator};
    let stub = Stub::start();
    let store = oxigraph::store::Store::new().unwrap();
    let text = format!(
        "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ SERVICE <{}> {{ ?s ?p ?o }} }}",
        stub.url()
    );
    let results = SparqlEvaluator::new()
        .parse_query(&text)
        .unwrap()
        .on_store(&store)
        .execute()
        .unwrap();
    if let QueryResults::Graph(triples) = results {
        for t in triples {
            t.unwrap();
        }
    }
    assert_eq!(
        stub.requests().len(),
        1,
        "raw oxigraph did not reach the stub"
    );
}
