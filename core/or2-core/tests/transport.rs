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
