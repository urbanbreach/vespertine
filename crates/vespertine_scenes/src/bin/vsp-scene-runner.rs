//! Vespertine scene runner (`specs/scenes/PROTOCOL.md`).

fn main() -> std::process::ExitCode {
    vespertine_scenes::run(vespertine_platform::application)
}
