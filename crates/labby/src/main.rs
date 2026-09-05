#![recursion_limit = "256"]

use std::process::ExitCode;
use std::time::Duration;

const CLI_STACK_SIZE: usize = 8 * 1024 * 1024;
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

fn run() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start labby async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let exit_code = runtime.block_on(labby::run());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    exit_code
}

fn main() -> ExitCode {
    let runner = std::thread::Builder::new()
        .name("labby-main".into())
        .stack_size(CLI_STACK_SIZE)
        .spawn(run);
    match runner {
        Ok(runner) => runner.join().unwrap_or_else(|_| {
            eprintln!("labby runtime thread panicked");
            ExitCode::FAILURE
        }),
        Err(error) => {
            eprintln!("failed to start labby runtime thread: {error}");
            ExitCode::FAILURE
        }
    }
}
