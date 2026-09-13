//! Generic runner shell. Adopting binaries populate a TargetRegistry and call
//! the same run_cli function. This shell deliberately has no product model.

fn main() {
    let registry = verify_runner::TargetRegistry::default();
    let code = verify_runner::run_cli(
        &registry,
        std::env::args_os().skip(1),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
