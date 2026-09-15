//! Registry-populated CLI demonstration. This counter is a harness self-test,
//! not a Labby model or a product-correctness claim.

#[path = "../tests/fixtures/counter.rs"]
#[allow(dead_code)]
mod counter;

fn main() {
    let registry = counter::registry(counter::Counter::default(), "safety");
    std::process::exit(verify_runner::run_cli(
        &registry,
        std::env::args_os().skip(1),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    ));
}
