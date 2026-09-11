//! Report contract and renderers for verification results.
//!
//! The JSON form is the stable contract; text, Markdown, and HTML all render
//! from it rather than being generated independently.
//!
//! This crate is nominally an M5 milestone, but `verify-runner` depends on it
//! (SPEC §3) and `ReplayReport` has to live somewhere from M2 onward. It starts
//! minimal — the replay report plus a text renderer — so that adding coverage
//! matrices later does not mean moving a public type out of the runner and
//! breaking every caller.
//!
//! Contents arrive in M2.
