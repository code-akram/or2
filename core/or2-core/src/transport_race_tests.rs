//! Address racing against a scripted fake transport on tokio's paused clock, so every start
//! time is exact.

use std::sync::Mutex;

use tokio::io::DuplexStream;

use super::*;

/// What the fake transport does when asked to connect to address `port - 1`.
#[derive(Clone, Copy)]
enum Behavior {
    Succeed(u64),
    Fail(u64),
    Hang,
}

struct Fake {
    script: Vec<Behavior>,
    origin: Instant,
    /// `(address index, start time)` in start order.
    started: Mutex<Vec<(usize, Duration)>>,
    /// Attempts dropped before they finished.
    cancelled: Mutex<Vec<usize>>,
}

struct CancelGuard<'a> {
    index: usize,
    log: &'a Mutex<Vec<usize>>,
    finished: bool,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.log.lock().unwrap().push(self.index);
        }
    }
}

impl Fake {
    fn new(script: Vec<Behavior>) -> Arc<Self> {
        Arc::new(Self {
            script,
            origin: Instant::now(),
            started: Mutex::default(),
            cancelled: Mutex::default(),
        })
    }

    fn addresses(&self) -> Vec<Endpoint> {
        (1..=self.script.len())
            .map(|port| Endpoint::new("fake", port as u16).unwrap())
            .collect()
    }

    fn started(&self) -> Vec<(usize, u64)> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .map(|(index, at)| (*index, at.as_millis() as u64))
            .collect()
    }

    fn cancelled(&self) -> Vec<usize> {
        let mut cancelled = self.cancelled.lock().unwrap().clone();
        cancelled.sort_unstable();
        cancelled
    }
}

impl Transport for Fake {
    type Stream = DuplexStream;

    async fn connect(&self, endpoint: &Endpoint) -> io::Result<DuplexStream> {
        let index = usize::from(endpoint.port()) - 1;
        self.started
            .lock()
            .unwrap()
            .push((index, self.origin.elapsed()));
        let mut guard = CancelGuard {
            index,
            log: &self.cancelled,
            finished: false,
        };
        let result = match self.script[index] {
            Behavior::Succeed(ms) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(tokio::io::duplex(8).0)
            }
            Behavior::Fail(ms) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("refused {index}"),
                ))
            }
            Behavior::Hang => std::future::pending().await,
        };
        guard.finished = true;
        result
    }
}

async fn run(fake: &Arc<Fake>) -> (Result<usize, RaceFailure>, u64) {
    let start = Instant::now();
    let result = race(fake, &fake.addresses(), RACE_STAGGER).await;
    (
        result.map(|raced| raced.index),
        start.elapsed().as_millis() as u64,
    )
}

#[tokio::test(start_paused = true)]
async fn the_first_address_wins_alone_when_it_answers_quickly() {
    let fake = Fake::new(vec![
        Behavior::Succeed(100),
        Behavior::Succeed(0),
        Behavior::Succeed(0),
    ]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 0);
    assert_eq!(elapsed, 100);
    assert_eq!(fake.started(), [(0, 0)], "later addresses never start");
}

#[tokio::test(start_paused = true)]
async fn a_silent_address_is_overtaken_250ms_later_and_the_loser_is_dropped() {
    let fake = Fake::new(vec![
        Behavior::Hang,
        Behavior::Succeed(100),
        Behavior::Succeed(0),
    ]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 1);
    assert_eq!(elapsed, 350);
    assert_eq!(fake.started(), [(0, 0), (1, 250)]);
    tokio::task::yield_now().await;
    assert_eq!(fake.cancelled(), [0]);
}

#[tokio::test(start_paused = true)]
async fn a_failure_starts_the_next_address_immediately() {
    let fake = Fake::new(vec![
        Behavior::Fail(40),
        Behavior::Fail(10),
        Behavior::Succeed(5),
    ]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 2);
    assert_eq!(fake.started(), [(0, 0), (1, 40), (2, 50)]);
    assert_eq!(elapsed, 55);
}

#[tokio::test(start_paused = true)]
async fn an_older_failure_does_not_pull_the_schedule_forward() {
    // Address 0 fails at 300 ms, after address 1 started at 250: address 2 still starts at
    // 500 ms (250 after address 1), not at 300.
    let fake = Fake::new(vec![
        Behavior::Fail(300),
        Behavior::Hang,
        Behavior::Succeed(10),
    ]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 2);
    assert_eq!(fake.started(), [(0, 0), (1, 250), (2, 500)]);
    assert_eq!(elapsed, 510);
}

#[tokio::test(start_paused = true)]
async fn a_slow_early_winner_beats_later_attempts_which_are_all_cancelled() {
    let fake = Fake::new(vec![
        Behavior::Succeed(600),
        Behavior::Hang,
        Behavior::Hang,
        Behavior::Succeed(0),
    ]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 0);
    assert_eq!(elapsed, 600);
    assert_eq!(fake.started(), [(0, 0), (1, 250), (2, 500)]);
    tokio::task::yield_now().await;
    assert_eq!(fake.cancelled(), [1, 2]);
}

#[tokio::test(start_paused = true)]
async fn when_every_address_fails_each_error_is_listed_in_order() {
    let fake = Fake::new(vec![
        Behavior::Fail(30),
        Behavior::Fail(400),
        Behavior::Fail(0),
    ]);
    let (result, _) = run(&fake).await;
    let failure = result.unwrap_err();
    assert_eq!(failure.errors.len(), 3);
    assert_eq!(
        failure.to_string(),
        "address 0: connection refused; \
         address 1: connection refused; \
         address 2: connection refused"
    );
    assert!(fake.cancelled().is_empty());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_race_cancels_every_attempt() {
    let fake = Fake::new(vec![Behavior::Hang, Behavior::Hang]);
    let addresses = fake.addresses();
    let result = tokio::time::timeout(
        Duration::from_millis(300),
        race(&fake, &addresses, RACE_STAGGER),
    )
    .await;
    assert!(result.is_err());
    // Aborted tasks drop their futures when the runtime next polls them.
    tokio::task::yield_now().await;
    assert_eq!(fake.cancelled(), [0, 1]);
}

#[tokio::test(start_paused = true)]
async fn an_address_that_never_answers_fails_at_its_own_timeout_not_the_overall_one() {
    // Address 0 is blackholed, address 1 refuses at once: the race ends when the silent one
    // gives up, 6 s after it started, with both outcomes.
    let fake = Fake::new(vec![Behavior::Hang, Behavior::Fail(0)]);
    let (result, elapsed) = run(&fake).await;
    let failure = result.unwrap_err();
    assert_eq!(elapsed, 6_000);
    assert_eq!(
        failure.to_string(),
        "address 0: no answer within 6 s; address 1: connection refused"
    );
}

#[tokio::test(start_paused = true)]
async fn the_per_address_timeouts_run_side_by_side_not_one_after_another() {
    let fake = Fake::new(vec![Behavior::Hang, Behavior::Hang, Behavior::Hang]);
    let (result, elapsed) = run(&fake).await;
    // Address 2 starts at 500 ms and gives up at 6.5 s; nothing waited for address 0 first.
    assert_eq!(elapsed, 6_500);
    assert_eq!(
        result.unwrap_err().to_string(),
        "address 0: no answer within 6 s; address 1: no answer within 6 s; \
         address 2: no answer within 6 s"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_winner_inside_its_allowance_still_wins() {
    let fake = Fake::new(vec![Behavior::Hang, Behavior::Succeed(5_500)]);
    let (result, elapsed) = run(&fake).await;
    assert_eq!(result.unwrap(), 1);
    assert_eq!(elapsed, 5_750);
}

#[tokio::test(start_paused = true)]
async fn the_report_follows_every_address_and_explains_a_race_that_is_still_running() {
    let fake = Fake::new(vec![Behavior::Fail(100), Behavior::Hang]);
    let report = RaceReport::new();
    assert!(!report.is_pending(), "nothing has started");
    let addresses = fake.addresses();
    let race = race_with(&fake, &addresses, RaceTiming::default(), Some(&report));
    tokio::pin!(race);
    // Poll the race for 2.5 s of virtual time, then ask what each address is doing.
    let _ = tokio::time::timeout(Duration::from_millis(2_500), &mut race).await;
    assert!(report.is_pending());
    assert_eq!(
        report.describe(),
        "address 0: connection refused; address 1: still trying after 2.4 s"
    );
    // Run it out: the report ends with the race.
    let _ = race.await;
    assert!(!report.is_pending());
    assert_eq!(
        report.describe(),
        "address 0: connection refused; address 1: no answer within 6 s"
    );
}

#[test]
fn errors_are_described_without_names_and_unreachable_ones_say_there_is_no_route() {
    use io::ErrorKind::*;
    let os = |code| io::Error::from_raw_os_error(code);
    assert_eq!(describe_error(&os(111)), "connection refused");
    // ENETUNREACH (101) and EHOSTUNREACH (113).
    assert_eq!(describe_error(&os(101)), "no route to the host");
    assert_eq!(describe_error(&os(113)), "no route to the host");
    assert_eq!(
        describe_error(&io::Error::from(NetworkUnreachable)),
        "no route to the host"
    );
    assert_eq!(describe_error(&os(110)), "no answer");
    assert_eq!(
        describe_error(&io::Error::new(TimedOut, "no answer within 6 s")),
        "no answer within 6 s"
    );
    assert_eq!(seconds(Duration::from_millis(5_500)), "5.5 s");
}

#[tokio::test(start_paused = true)]
async fn a_no_route_failure_hands_over_to_the_next_address_at_once() {
    struct NoRoute;
    impl Transport for NoRoute {
        type Stream = DuplexStream;
        async fn connect(&self, endpoint: &Endpoint) -> io::Result<DuplexStream> {
            if endpoint.port() == 1 {
                Err(io::Error::from_raw_os_error(101))
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(tokio::io::duplex(8).0)
            }
        }
    }
    let transport = Arc::new(NoRoute);
    let addresses = [
        Endpoint::new("a", 1).unwrap(),
        Endpoint::new("b", 2).unwrap(),
    ];
    let start = Instant::now();
    let raced = race(&transport, &addresses, RACE_STAGGER).await.unwrap();
    assert_eq!(raced.index, 1);
    assert_eq!(
        start.elapsed(),
        Duration::from_millis(10),
        "no stagger wait"
    );
}
