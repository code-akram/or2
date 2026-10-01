//! DirectTcp against local listeners, and proof that a Transport stream feeds russh.

use std::io::ErrorKind;
use std::sync::Arc;

use or2_core::transport::{DirectTcp, Endpoint, Transport};
use russh::client;
use russh::keys::PublicKeyOrCertificate;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn listener() -> (TcpListener, Endpoint) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, Endpoint::new("127.0.0.1", port).unwrap())
}

#[tokio::test]
async fn direct_tcp_carries_bytes_both_ways_without_nagle() {
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4];
        socket.read_exact(&mut buf).await.unwrap();
        socket
            .write_all(&buf.map(|b| b.to_ascii_uppercase()))
            .await
            .unwrap();
    });
    let mut stream = DirectTcp.connect(&endpoint).await.unwrap();
    assert!(stream.nodelay().unwrap());
    stream.write_all(b"ping").await.unwrap();
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"PING");
    server.await.unwrap();
}

#[tokio::test]
async fn direct_tcp_reports_refused_connections() {
    let (listener, endpoint) = listener().await;
    drop(listener);
    let error = DirectTcp.connect(&endpoint).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
}

#[tokio::test]
async fn a_race_reports_the_address_the_winning_connection_reached() {
    let (dead, dead_endpoint) = listener().await;
    drop(dead);
    let (live, live_endpoint) = listener().await;
    let live_addr = live.local_addr().unwrap();
    let raced = or2_core::transport::race(
        &Arc::new(DirectTcp),
        &[dead_endpoint, live_endpoint],
        std::time::Duration::from_millis(10),
    )
    .await
    .unwrap();
    assert_eq!(raced.index, 1);
    assert_eq!(raced.peer, Some(live_addr));
    assert_eq!(DirectTcp.peer_addr(&raced.stream), Some(live_addr));
}

struct RejectAll;

impl client::Handler for RejectAll {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(false)
    }
}

#[tokio::test]
async fn transport_stream_is_accepted_by_russh_connect_stream() {
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut client_id = vec![0u8; 8];
        socket.read_exact(&mut client_id).await.unwrap();
        // Not an SSH server: russh must fail cleanly on the identification line.
        socket
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await
            .unwrap();
        client_id
    });
    let stream = DirectTcp.connect(&endpoint).await.unwrap();
    let result =
        client::connect_stream(Arc::new(client::Config::default()), stream, RejectAll).await;
    assert!(result.is_err(), "a non-SSH peer must not produce a session");
    assert_eq!(&server.await.unwrap(), b"SSH-2.0-");
}

#[tokio::test]
async fn race_skips_a_refused_address_at_once_and_reports_every_failure() {
    use or2_core::transport::{RACE_STAGGER, race};
    let transport = Arc::new(DirectTcp);
    let (dead, dead_endpoint) = listener().await;
    drop(dead);
    let (live, live_endpoint) = listener().await;
    let accept = tokio::spawn(async move { live.accept().await.unwrap().0 });

    // A refusal does not wait out the stagger: the live address starts as soon as the dead
    // one fails, so the whole race is far quicker than 250 ms.
    let started = std::time::Instant::now();
    let raced = race(
        &transport,
        &[dead_endpoint.clone(), live_endpoint.clone()],
        RACE_STAGGER,
    )
    .await
    .unwrap();
    assert_eq!(raced.index, 1);
    assert!(raced.stream.nodelay().unwrap());
    assert!(started.elapsed() < RACE_STAGGER, "{:?}", started.elapsed());
    accept.await.unwrap();

    // All dead: every address's error is listed, in request order.
    let failure = race(
        &transport,
        &[dead_endpoint.clone(), dead_endpoint],
        RACE_STAGGER,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.errors.len(), 2);
    assert!(
        failure
            .errors
            .iter()
            .all(|error| error.kind() == ErrorKind::ConnectionRefused)
    );
    let text = failure.to_string();
    assert!(text.contains("address 0: ConnectionRefused"), "{text}");
    assert!(text.contains("address 1: ConnectionRefused"), "{text}");
}
