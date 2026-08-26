// A panic anywhere in here takes down the daemon's single forwarding-loop
// task, not just one bad frame/request — every fallible path must return a
// Result, never unwrap/expect its way out. Scoped to this crate root (not
// tests/, where a panicking .unwrap() on a failed assertion is correct)
// since inner attributes don't cross into the separate integration-test
// crates under tests/.
#![warn(clippy::unwrap_used, clippy::expect_used)]

pub mod config;
pub mod daemon;
pub mod frame;
pub mod io;
pub mod switch;
