//! Regressions from the external fix check of 6afa42e (ported from its probes); fixtures only.
mod common;
use common::*;
use or2_core::pair::{PairError, PairOffer};
use or2_core::transport::Endpoint;
use or2_pair::confirm::Answer;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

#[test]
fn a_signed_success_relabelled_as_a_refusal_is_not_authentic() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(
        &world,
        &options(),
        &confirm,
        Duration::from_secs(3),
        |ready| {
            let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = proxy.local_addr().unwrap();
            let bridge = std::thread::spawn(move || {
                let (mut phone, _) = proxy.accept().unwrap();
                phone
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut host = TcpStream::connect(ready.listening[0]).unwrap();
                host.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut host_reader = BufReader::new(host.try_clone().unwrap());
                let mut phone_reader = BufReader::new(phone.try_clone().unwrap());
                let mut line = String::new();
                host_reader.read_line(&mut line).unwrap();
                phone.write_all(line.as_bytes()).unwrap();
                line.clear();
                phone_reader.read_line(&mut line).unwrap();
                host.write_all(line.as_bytes()).unwrap();
                line.clear();
                host_reader.read_line(&mut line).unwrap();
                let mut reply: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(reply["ok"], true);
                reply["ok"] = false.into();
                reply["reason"] = "ok".into();
                // An attacker retains the host's MAC without knowing the OTP.
                phone.write_all(format!("{reply}\n").as_bytes()).unwrap();
            });
            let mut offer = PairOffer::parse(&ready.payload).unwrap();
            offer.exchange.as_mut().unwrap().endpoints =
                vec![Endpoint::new("127.0.0.1", addr.port()).unwrap()];
            let result = phone_submits(&offer, &key, "phone");
            bridge.join().unwrap();
            result
        },
    );
    // The relabelled answer proves nothing: the phone takes it for a forgery (the code is not
    // spent), not for a refusal. Under v2 it verified and the phone reported `Refused(Other)`.
    assert_eq!(result.phone.unwrap(), Err(PairError::HostNotAuthenticated));
    assert!(world.authorized_keys().unwrap().contains(&key));
    assert_eq!(confirm.asked.lock().unwrap().len(), 1);
}

#[test]
fn five_probes_block_an_honest_phone_sharing_the_peer_address() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(
        &world,
        &options(),
        &confirm,
        Duration::from_secs(2),
        |ready| {
            for _ in 0..5 {
                let mut socket = TcpStream::connect(ready.listening[0]).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut hello = String::new();
                reader.read_line(&mut hello).unwrap();
                assert!(hello.contains("nonce"));
                socket.write_all(b"\n").unwrap();
                let mut refusal = String::new();
                reader.read_line(&mut refusal).unwrap();
                assert!(refusal.contains("request"));
            }
            std::thread::sleep(Duration::from_millis(100));
            phone_pairs(&ready.payload, &key, "honest-phone")
        },
    );
    assert_eq!(result.phone.unwrap(), Err(PairError::ConnectionLost));
    assert!(world.authorized_keys().is_none());
    assert!(confirm.asked.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_cooperating_writer_lock_is_respected_without_modification() {
    use or2_pair::{account::Account, authorized_keys, date::DateTime, keyline::KeyLine};
    use std::os::fd::AsRawFd;
    let world = World::new();
    let path = world.home.path().join(".ssh/authorized_keys");
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("{HOST_KEY}\n")).unwrap();
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let error = authorized_keys::add(
        &Account::new("tester", world.home.path()),
        &KeyLine::parse(&phone_key()).unwrap(),
        "phone",
        DateTime::from_unix(1_782_867_661),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{HOST_KEY}\n")
    );
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}
