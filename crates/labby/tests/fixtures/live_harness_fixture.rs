#![allow(clippy::panic)]

use std::io::Write as _;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .unwrap_or_default();
    let port = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(|arg| arg.parse::<u16>().ok())
        .expect("fixture port");
    let marker = args.next().map(PathBuf::from).expect("fixture marker");
    match mode.as_str() {
        "grandchild-listener" => {
            #[allow(clippy::zombie_processes)]
            let child = Command::new(std::env::current_exe().expect("fixture executable"))
                .args(["child-listener", &port.to_string()])
                .arg(&marker)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn grandchild listener");
            std::fs::write(&marker, child.id().to_string()).expect("write grandchild marker");
            loop {
                std::thread::sleep(Duration::from_mins(1));
            }
        }
        "child-listener" => {
            let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind held listener");
            while !marker.exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::fs::OpenOptions::new()
                .append(true)
                .open(&marker)
                .ok()
                .and_then(|mut file| writeln!(file, " ready").ok());
            let _listener = listener;
            loop {
                std::thread::sleep(Duration::from_mins(1));
            }
        }
        _ => panic!("unknown fixture mode"),
    }
}
