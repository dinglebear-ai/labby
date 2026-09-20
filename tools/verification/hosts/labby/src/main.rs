//! Labby-owned verification host.

use std::{env, ffi::OsString, io};

fn main() {
    let code = labby_verify::run(
        env::args_os().skip(1).collect::<Vec<OsString>>(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    std::process::exit(code);
}
