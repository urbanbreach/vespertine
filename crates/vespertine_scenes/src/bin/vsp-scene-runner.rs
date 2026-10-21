//! Vespertine scene runner (see the crate README for the command line and modes).

fn main() -> std::process::ExitCode {
    vespertine_scenes::run(vespertine_platform::application)
}
