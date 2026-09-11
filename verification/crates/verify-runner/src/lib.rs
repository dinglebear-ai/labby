//! Target registry, replay engine, and the `verify` command line.
//!
//! Replay is the common denominator of the whole toolkit: every counterexample
//! origin — model checker, fuzzer, or production incident — reduces to a
//! scenario file that this engine executes. It is pure, deterministic, and
//! needs no external toolchain, which is why it ships before any backend.
//!
//! This crate depends on **no backend crate**. The `Backend` trait lives in
//! `verify-core`, and an adopting project registers the backend implementations
//! it wants. That inversion is what lets a project take Stateright without
//! taking a Java toolchain.
//!
//! Contents arrive in M2.
