//! Process fixture used by the owned live-test harness.

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
            let _child = Command::new(std::env::current_exe().expect("fixture executable"))
                .args(["child-listener", &port.to_string()])
                .arg(&marker)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn grandchild listener");
            loop {
                std::thread::sleep(Duration::from_mins(1));
            }
        }
        "child-listener" => {
            let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind held listener");
            let address = listener.local_addr().expect("held listener address");
            let mut file = std::fs::File::create(&marker).expect("create grandchild marker");
            writeln!(file, "{} {} ready", std::process::id(), address.port())
                .expect("write grandchild marker");
            file.sync_all().expect("sync grandchild marker");
            let _listener = listener;
            loop {
                std::thread::sleep(Duration::from_mins(1));
            }
        }
        _ => panic!("unknown fixture mode"),
    }
}
