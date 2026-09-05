use std::process::ExitCode;

const CLI_STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    let runner = std::thread::Builder::new()
        .name("labby-main".into())
        .stack_size(CLI_STACK_SIZE)
        .spawn(labby::run);
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
