//! Lane C executable proposal; production wiring waits for the Gate 0/A/B handoff.
#[path = "issue_771_server_lifecycle/fencing.rs"]
mod fencing;
#[path = "issue_771_server_lifecycle/snapshot.rs"]
mod snapshot;
#[path = "issue_771_server_lifecycle/snapshot_tests.rs"]
mod snapshot_tests;
#[path = "issue_771_server_lifecycle/transport_matrix.rs"]
mod transport_matrix;
