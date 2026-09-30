//! Lane F black-box regressions against the public gateway API.
//! No source inclusion, private-field access, or shared testkit modifications.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "issue_771_adversarial/delivery.rs"]
mod delivery;
#[path = "issue_771_adversarial/fixture.rs"]
mod fixture;
#[path = "issue_771_adversarial/routing.rs"]
mod routing;
