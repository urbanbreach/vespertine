//! Renders text in a font registered at run time (TXT-003).
//!
//! Runs headless and exits with a panic if a check fails.

#[path = "../tests/support/registered_font_scenario.rs"]
mod scenario;

fn main() {
    scenario::run();
    println!("registered_font: ok");
}
