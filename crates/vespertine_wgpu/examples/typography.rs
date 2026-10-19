//! Shows interface, code and terminal typography roles and changes the interface size (TXT-004).
//!
//! Runs headless and exits with a panic if a check fails.

#[path = "../tests/support/typography_scenario.rs"]
mod scenario;

fn main() {
    scenario::run();
    println!("typography: ok");
}
