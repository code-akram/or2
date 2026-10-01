//! Resolving and dialing against a scripted resolver and connector on tokio's paused clock, so
//! every start time and every wait is exact.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::*;

fn v4(last: u8) -> SocketAddr {
    SocketAddr::from(([192, 0, 2, last], 22))
}

fn v6(last: u16) -> SocketAddr {
    SocketAddr::from(([0x2001, 0xdb8, 0, 0, 0, 0, 0, last], 22))
}

/// `fe80::1` with the given scope id.
fn link_local(scope: u32) -> SocketAddr {
    SocketAddr::V6(std::net::SocketAddrV6::new(
        "fe80::1".parse().unwrap(),
        22,
        0,
        scope,
    ))
}

/// One scripted answer of the resolver: after `after`, either these addresses or an error.
struct Answer {
    after: Duration,
    result: Result<Vec<SocketAddr>, io::ErrorKind>,
}

fn answer(after_ms: u64, result: Result<Vec<SocketAddr>, io::ErrorKind>) -> Answer {
    Answer {
        after: Duration::from_millis(after_ms),
        result,
    }
}

/// Answers each call from the script in order (the last answer repeats), and remembers when it
/// was asked.
struct FakeResolver {
    origin: Instant,
    script: Mutex<VecDeque<Answer>>,
    asked: Mutex<Vec<Duration>>,
}

impl FakeResolver {
    fn new(script: Vec<Answer>) -> Self {
        Self {
            origin: Instant::now(),
            script: Mutex::new(script.into()),
            asked: Mutex::default(),
        }
    }

    fn asked(&self) -> Vec<u64> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|at| at.as_millis() as u64)
            .collect()
    }
}

impl Resolver for FakeResolver {
    async fn resolve(&self, _: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
        self.asked.lock().unwrap().push(self.origin.elapsed());
        let answer = {
            let mut script = self.script.lock().unwrap();
            if script.len() > 1 {
                script.pop_front().unwrap()
            } else {
                let last = script.front().expect("a scripted answer");
                Answer {
                    after: last.after,
                    result: last.result.clone(),
                }
            }
        };
        sleep(answer.after).await;
        answer
            .result
            .map_err(|kind| io::Error::new(kind, "the resolver failed"))
    }
}

#[derive(Clone, Copy)]
enum Dial {
    Succeed(u64),
    Refuse,
    Hang,
}

/// A connector that does what the script says for each address, and remembers when each
/// address was started.
#[derive(Clone)]
struct FakeConnector {
    origin: Instant,
    behavior: Arc<Vec<(SocketAddr, Dial)>>,
    started: Arc<Mutex<Vec<(SocketAddr, u64)>>>,
}

impl FakeConnector {
    fn new(behavior: Vec<(SocketAddr, Dial)>) -> Self {
        Self {
            origin: Instant::now(),
            behavior: Arc::new(behavior),
            started: Arc::default(),
        }
    }

    fn started(&self) -> Vec<(SocketAddr, u64)> {
        self.started.lock().unwrap().clone()
    }
}

impl Connector for FakeConnector {
    type Stream = ();

    async fn connect(&self, address: SocketAddr) -> io::Result<()> {
        self.started
            .lock()
            .unwrap()
            .push((address, self.origin.elapsed().as_millis() as u64));
        let behavior = self
            .behavior
            .iter()
            .find(|(candidate, _)| *candidate == address)
            .map_or(Dial::Hang, |(_, dial)| *dial);
        match behavior {
            Dial::Succeed(ms) => {
                sleep(Duration::from_millis(ms)).await;
                Ok(())
            }
            Dial::Refuse => Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
            Dial::Hang => std::future::pending().await,
        }
    }
}

fn endpoint(host: &str) -> Endpoint {
    Endpoint::new(host, 22).unwrap()
}

async fn dial_with(
    resolver: &FakeResolver,
    connector: &FakeConnector,
    host: &str,
) -> (io::Result<SocketAddr>, u64) {
    let start = Instant::now();
    let result = dial(resolver, connector, &endpoint(host), DialTiming::default()).await;
    (
        result.map(|(_, address)| address),
        start.elapsed().as_millis() as u64,
    )
}

#[test]
fn only_dot_local_names_are_mdns_names() {
    for name in ["blackstark.local", "Mac.LOCAL", "mac.local."] {
        assert!(is_local_name(name), "{name}");
    }
    for name in [
        "local",
        "mac.localhost",
        "mac.example.org",
        "10.0.0.1",
        "fe80::1",
    ] {
        assert!(!is_local_name(name), "{name}");
    }
}

#[test]
fn addresses_alternate_families_starting_with_the_resolvers_first() {
    let order = |addresses: Vec<SocketAddr>| interleave(addresses);
    assert_eq!(
        order(vec![v6(1), v6(2), v4(1), v4(2), v4(3)]),
        [v6(1), v4(1), v6(2), v4(2), v4(3)]
    );
    assert_eq!(order(vec![v4(1), v4(2), v6(1)]), [v4(1), v6(1), v4(2)]);
    assert_eq!(order(vec![v4(1)]), [v4(1)]);
    assert!(order(Vec::new()).is_empty());
}

#[test]
fn only_an_unscoped_ipv6_link_local_address_is_unusable() {
    assert!(is_unscoped_link_local(&link_local(0)));
    assert!(
        !is_unscoped_link_local(&link_local(3)),
        "a scope makes it usable"
    );
    assert!(!is_unscoped_link_local(&v6(1)));
    assert!(!is_unscoped_link_local(&v4(1)));
    // fe80::/10 reaches febf::.
    let upper: SocketAddr = "[febf::1]:22".parse().unwrap();
    assert!(is_unscoped_link_local(&upper));
    let outside: SocketAddr = "[fec0::1]:22".parse().unwrap();
    assert!(!is_unscoped_link_local(&outside));
}

#[tokio::test(start_paused = true)]
async fn an_endpoint_resolves_once_and_the_first_address_wins_when_it_answers() {
    let resolver = FakeResolver::new(vec![answer(20, Ok(vec![v4(1), v4(2)]))]);
    let connector = FakeConnector::new(vec![(v4(1), Dial::Succeed(30)), (v4(2), Dial::Succeed(0))]);
    let (result, elapsed) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap(), v4(1));
    assert_eq!(elapsed, 50);
    assert_eq!(resolver.asked(), [0], "resolved once");
    assert_eq!(
        connector.started(),
        [(v4(1), 20)],
        "the second never started"
    );
}

#[tokio::test(start_paused = true)]
async fn resolved_addresses_race_happy_eyeballs_style_with_a_stagger() {
    // The first address (IPv6) is blackholed; IPv4 starts 250 ms later and wins.
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![v6(1), v6(2), v4(1)]))]);
    let connector = FakeConnector::new(vec![
        (v6(1), Dial::Hang),
        (v6(2), Dial::Hang),
        (v4(1), Dial::Succeed(40)),
    ]);
    let (result, elapsed) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap(), v4(1));
    // Families alternate: v6, v4 at 250 ms (wins at 290), the second v6 never starts.
    assert_eq!(connector.started(), [(v6(1), 0), (v4(1), 250)]);
    assert_eq!(elapsed, 290);
}

#[tokio::test(start_paused = true)]
async fn a_refused_address_starts_the_next_at_once() {
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![v4(1), v4(2)]))]);
    let connector = FakeConnector::new(vec![(v4(1), Dial::Refuse), (v4(2), Dial::Succeed(5))]);
    let (result, elapsed) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap(), v4(2));
    assert_eq!(elapsed, 5);
}

#[tokio::test(start_paused = true)]
async fn link_local_results_without_a_scope_are_skipped_not_failed_on() {
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![link_local(0), v4(1)]))]);
    let connector = FakeConnector::new(vec![(v4(1), Dial::Succeed(1))]);
    let (result, _) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap(), v4(1));
    assert_eq!(
        connector.started(),
        [(v4(1), 0)],
        "the link-local address was never tried"
    );

    // A scoped one is usable.
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![link_local(7)]))]);
    let connector = FakeConnector::new(vec![(link_local(7), Dial::Succeed(1))]);
    let (result, _) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap(), link_local(7));

    // Nothing but unscoped link-local addresses: said so, without trying any.
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![link_local(0)]))]);
    let connector = FakeConnector::new(Vec::new());
    let (result, _) = dial_with(&resolver, &connector, "mac.example.org").await;
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AddrNotAvailable);
    assert!(
        error
            .to_string()
            .contains("link-local address without a scope"),
        "{error}"
    );
    assert!(connector.started().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_dot_local_name_that_fails_to_resolve_is_tried_up_to_three_times() {
    // The first lookup of an app can fail after ~1.5 s and a later one succeed from the cache.
    let resolver = FakeResolver::new(vec![
        answer(1_500, Err(io::ErrorKind::NotFound)),
        answer(0, Ok(vec![v4(1)])),
    ]);
    let connector = FakeConnector::new(vec![(v4(1), Dial::Succeed(10))]);
    let (result, elapsed) = dial_with(&resolver, &connector, "blackstark.local").await;
    assert_eq!(result.unwrap(), v4(1));
    // 1.5 s for the failed try, the 250 ms pause, then an instant answer and the connect.
    assert_eq!(resolver.asked(), [0, 1_750]);
    assert_eq!(elapsed, 1_760);
}

#[tokio::test(start_paused = true)]
async fn a_dot_local_name_that_never_resolves_fails_after_three_tries_and_says_mdns() {
    let resolver = FakeResolver::new(vec![answer(600, Err(io::ErrorKind::NotFound))]);
    let connector = FakeConnector::new(Vec::new());
    let (result, elapsed) = dial_with(&resolver, &connector, "blackstark.local").await;
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.to_string(), "name not resolved (mDNS) after 3 tries");
    assert_eq!(resolver.asked(), [0, 850, 1_700]);
    assert_eq!(elapsed, 2_300);
    assert!(connector.started().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_dot_local_tries_stop_at_four_seconds_however_many_remain() {
    // Each try takes 1.9 s to fail: the second starts at 2.15 s and would fail at 4.05 s, past
    // the 4 s window, so it is cut there and no third one starts.
    let resolver = FakeResolver::new(vec![answer(1_900, Err(io::ErrorKind::NotFound))]);
    let connector = FakeConnector::new(Vec::new());
    let (result, elapsed) = dial_with(&resolver, &connector, "blackstark.local").await;
    assert_eq!(
        result.unwrap_err().to_string(),
        "name not resolved (mDNS) within 4 s"
    );
    assert_eq!(
        resolver.asked(),
        [0, 2_150],
        "the second try was cut at the window"
    );
    assert_eq!(elapsed, 4_000);

    // A try that never returns ends with the window.
    let resolver = FakeResolver::new(vec![answer(60_000, Ok(vec![v4(1)]))]);
    let (result, elapsed) = dial_with(&resolver, &FakeConnector::new(Vec::new()), "x.local").await;
    assert_eq!(
        result.unwrap_err().to_string(),
        "name not resolved (mDNS) within 4 s"
    );
    assert_eq!(elapsed, 4_000);
}

#[tokio::test(start_paused = true)]
async fn another_name_is_resolved_once_and_a_failure_is_a_plain_not_resolved() {
    let resolver = FakeResolver::new(vec![answer(100, Err(io::ErrorKind::NotFound))]);
    let connector = FakeConnector::new(Vec::new());
    let (result, elapsed) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap_err().to_string(), "name not resolved");
    assert_eq!(resolver.asked(), [0]);
    assert_eq!(elapsed, 100);

    // A resolver that returns nothing is a failure too.
    let resolver = FakeResolver::new(vec![answer(0, Ok(Vec::new()))]);
    let (result, _) = dial_with(&resolver, &connector, "mac.example.org").await;
    assert_eq!(result.unwrap_err().to_string(), "name not resolved");
}

#[tokio::test(start_paused = true)]
async fn the_whole_endpoint_is_bounded_by_its_budget_and_each_address_says_so() {
    let resolver = FakeResolver::new(vec![answer(200, Ok(vec![v6(1), v4(1)]))]);
    let connector = FakeConnector::new(vec![(v6(1), Dial::Hang), (v4(1), Dial::Hang)]);
    let (result, elapsed) = dial_with(&resolver, &connector, "mac.example.org").await;
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    // Both addresses gave up at the budget (5 s from the start, resolution included).
    assert_eq!(elapsed, 5_000);
    assert_eq!(error.to_string(), "2 addresses: no answer within 5 s");
}

#[tokio::test(start_paused = true)]
async fn mixed_failures_are_summarised_once_each() {
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![v6(1), v4(1), v4(2)]))]);
    let connector = FakeConnector::new(vec![
        (v6(1), Dial::Refuse),
        (v4(1), Dial::Refuse),
        (v4(2), Dial::Hang),
    ]);
    let (result, _) = dial_with(&resolver, &connector, "mac.example.org").await;
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    assert_eq!(
        error.to_string(),
        "3 addresses: connection refused, no answer within 5 s"
    );
}

#[tokio::test(start_paused = true)]
async fn a_dial_that_is_dropped_cancels_every_attempt() {
    let resolver = FakeResolver::new(vec![answer(0, Ok(vec![v4(1), v4(2)]))]);
    let connector = FakeConnector::new(vec![(v4(1), Dial::Hang), (v4(2), Dial::Hang)]);
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        dial(
            &resolver,
            &connector,
            &endpoint("mac.example.org"),
            DialTiming::default(),
        ),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        connector.started().len(),
        1,
        "the second had not started yet"
    );
}

#[tokio::test]
async fn the_system_resolver_resolves_ip_literals_and_localhost() {
    let literal = SystemResolver.resolve("127.0.0.1", 2222).await.unwrap();
    assert_eq!(literal, ["127.0.0.1:2222".parse::<SocketAddr>().unwrap()]);
    assert!(
        SystemResolver
            .resolve("localhost", 22)
            .await
            .unwrap()
            .iter()
            .all(|a| a.ip().is_loopback())
    );
    assert!(SystemResolver.resolve("host.invalid", 22).await.is_err());
}
