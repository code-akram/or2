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
        "address 0: ConnectionRefused (refused 0); \
         address 1: ConnectionRefused (refused 1); \
         address 2: ConnectionRefused (refused 2)"
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
