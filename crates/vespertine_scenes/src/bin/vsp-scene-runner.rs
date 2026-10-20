//! Vespertine scene runner (see the crate README for the command line and modes).

#[cfg(feature = "profile")]
#[path = "vsp-scene-runner/profile.rs"]
mod profile;

fn main() -> std::process::ExitCode {
    #[cfg(feature = "profile")]
    let profiler = profile::start();
    let code = vespertine_scenes::run(vespertine_platform::application);
    #[cfg(feature = "profile")]
    profile::finish(profiler);
    code
}
