use mosh_rs::{Base64Key, MoshSession};
use std::process::Command;
use std::time::{Duration, Instant};

fn wait_for(session: &mut MoshSession<mosh_rs::screen::Vt100Screen>, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        session.pump().expect("pump failed");
        let _ = session.render();
        if session.screen_text().contains(text) {
            return;
        }
    }
    panic!(
        "screen did not contain {text:?}; screen:\n{}",
        session.screen_text()
    );
}

fn main() {
    let output = Command::new("mosh-server")
        .env("LANG", "C.UTF-8")
        .args([
            "new",
            "-i",
            "0.0.0.0",
            "-p",
            "60101",
            "--",
            "/bin/bash",
            "--norc",
            "--noprofile",
        ])
        .output()
        .expect("start mosh-server");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("server stdout:\n{stdout}server stderr:\n{stderr}");
    let connect = stdout
        .lines()
        .find(|line| line.starts_with("MOSH CONNECT "))
        .expect("MOSH CONNECT line");
    let mut fields = connect.split_whitespace();
    let _ = fields.next();
    let _ = fields.next();
    let port: u16 = fields.next().unwrap().parse().unwrap();
    let key = Base64Key::from_printable(fields.next().unwrap()).unwrap();
    let pid: i32 = stderr
        .lines()
        .find_map(|line| {
            line.strip_prefix("[mosh-server detached, pid = ")
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse().ok())
        })
        .expect("server pid on stderr");
    println!("SPIKE_SERVER_PID={pid}");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut session = MoshSession::connect("127.0.0.1", port, &key).expect("connect");
        session.send_resize(80, 24);
        session.send_input(b"echo hello-or2-$((6*7))\r");
        wait_for(&mut session, "hello-or2-42");
        session.send_resize(100, 30);
        session.send_input(b"printf 'SIZE:%s\\n' \"$(stty size)\"\r");
        wait_for(&mut session, "SIZE:30 100");
        let old_port = session.local_port().unwrap();
        session.roam_now();
        let new_port = session.local_port().unwrap();
        assert_ne!(old_port, new_port, "roam_now did not rebind");
        session.send_input(b"echo roaming-or2-ok\r");
        wait_for(&mut session, "roaming-or2-ok");
        println!("PASS interop resize roaming; source port {old_port} -> {new_port}");
        session.shutdown();
    }));
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .expect("kill server");
    println!("cleanup kill status={status}");
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
