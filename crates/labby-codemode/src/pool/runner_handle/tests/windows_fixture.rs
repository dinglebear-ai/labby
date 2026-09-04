//! Native subprocess fixture, invoked only by the Windows containment test.

use std::io::Write as _;
use std::process::{Command, Stdio};

#[test]
#[ignore = "subprocess fixture invoked by shutdown_reaps_the_runner_and_its_descendant"]
fn runner_descendant() {
    let mut child = Command::new(std::env::current_exe().expect("fixture executable"))
        .args([
            "--exact",
            "pool::runner_handle::tests::windows_fixture::parked_descendant",
            "--ignored",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn native descendant");
    // The libtest harness may have printed a test-name prefix without a newline.
    println!("\nLABBY_DESCENDANT_PID={}", child.id());
    std::io::stdout().flush().expect("publish descendant pid");
    child.wait().expect("wait for supervised descendant");
}

#[test]
#[ignore = "native descendant invoked by runner_descendant"]
fn parked_descendant() {
    loop {
        std::thread::park();
    }
}
