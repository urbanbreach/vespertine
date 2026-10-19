//! PLT-012 example: prints the capability record, requests one window
//! capture and prints the distinct outcome (image, denied, cancelled, ...).
//! No other capture method is ever tried after a portal refusal.
//!
//! Usage:
//!   capture_service [--delay SECONDS] [--listen [COMBINATION]]
//!
//! `--delay` waits before capturing (to focus another window). `--listen`
//! registers the global shortcut (default `ctrl-alt-space`) and captures on
//! every activation until interrupted.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn main() {
    linux::run();
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn main() {
    eprintln!("capture_service runs on Linux (Wayland or X11) only");
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use std::{sync::Arc, time::Duration};

    use vespertine_desktop::{
        capture::{
            CaptureBackend, CaptureError, CaptureService, CaptureTarget, OwnWindowProbe,
            portal::ZbusScreenshotPortal, x11::X11rbServer,
        },
        shortcut::{
            Shortcut, ShortcutDriver, ShortcutService,
            wayland::{AshpdGlobalShortcuts, WaylandShortcutRegistration},
            x11::{X11ShortcutRegistration, X11rbKeyGrabber},
        },
    };

    /// This example owns no windows.
    struct NoOwnWindows;

    impl OwnWindowProbe for NoOwnWindows {
        fn own_window_is_active(&self) -> bool {
            false
        }
    }

    struct Options {
        delay: Duration,
        listen: Option<Shortcut>,
    }

    fn parse_options() -> Options {
        let mut options = Options {
            delay: Duration::ZERO,
            listen: None,
        };
        let default = Shortcut::parse("ctrl-alt-space").expect("valid default");
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--delay" => {
                    let seconds: u64 = args
                        .next()
                        .and_then(|value| value.parse().ok())
                        .expect("--delay needs a number of seconds");
                    options.delay = Duration::from_secs(seconds);
                }
                "--listen" => {
                    let combination = match args.peek() {
                        Some(next) if !next.starts_with("--") => args.next().unwrap(),
                        _ => String::new(),
                    };
                    options.listen = Some(Shortcut::parse_or(&combination, default.clone()));
                }
                other => panic!("unknown argument {other}"),
            }
        }
        options
    }

    fn backend() -> CaptureBackend {
        let portal = Arc::new(
            smol::block_on(ZbusScreenshotPortal::connect()).unwrap_or_else(|error| {
                eprintln!("cannot reach the session bus: {error}");
                std::process::exit(1)
            }),
        );
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            CaptureBackend::Wayland {
                portal,
                own_windows: Arc::new(NoOwnWindows),
            }
        } else if std::env::var_os("DISPLAY").is_some() {
            let server = X11rbServer::connect().unwrap_or_else(|error| {
                eprintln!("cannot connect to the X server: {error}");
                std::process::exit(1)
            });
            CaptureBackend::X11 {
                portal,
                server: Arc::new(server),
            }
        } else {
            eprintln!("neither WAYLAND_DISPLAY nor DISPLAY is set");
            std::process::exit(1)
        }
    }

    fn report(result: Result<vespertine_desktop::capture::CapturedImage, CaptureError>) {
        match result {
            Ok(image) => {
                let path = std::env::temp_dir().join(format!("{}.png", image.id));
                match std::fs::write(&path, &image.png) {
                    Ok(()) => println!(
                        "SUCCESS: saved {} bytes to {}",
                        image.png.len(),
                        path.display()
                    ),
                    Err(error) => println!("SUCCESS (not saved: {error})"),
                }
                println!(
                    "  {} x {}, target {:?} \"{}\" ({}), title {:?}, icon {}",
                    image.width,
                    image.height,
                    image.metadata.kind,
                    image.metadata.display_name,
                    image.metadata.identifier,
                    image.metadata.title,
                    image.metadata.icon_png.is_some(),
                );
                println!("  semantic snapshot: {:?}", image.semantic);
            }
            Err(CaptureError::PermissionDenied(reason)) => println!("DENIED: {reason}"),
            Err(CaptureError::Cancelled) => println!("CANCELLED: the user dismissed the portal"),
            Err(CaptureError::SelfCapture(message)) => println!("SELF-CAPTURE: {message}"),
            Err(CaptureError::NoEligibleWindow) => println!("NO ELIGIBLE WINDOW"),
            Err(CaptureError::ShortcutUnavailable(reason)) => {
                println!("SHORTCUT UNAVAILABLE: {reason}")
            }
            Err(CaptureError::Failed(reason)) => println!("FAILED: {reason}"),
        }
    }

    pub fn run() {
        let options = parse_options();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let shortcuts = options
            .listen
            .clone()
            .map(|combination| ShortcutService::new(combination, true));
        let mut service = CaptureService::new(backend());
        if let Some(shortcuts) = &shortcuts {
            service = service.with_shortcuts(shortcuts.clone());
        }

        smol::block_on(async {
            if let Some(shortcuts) = &shortcuts {
                let driver_service = shortcuts.clone();
                if wayland {
                    match AshpdGlobalShortcuts::connect().await {
                        Ok(portal) => {
                            let registration = WaylandShortcutRegistration::new(Box::new(portal));
                            smol::spawn(ShortcutDriver::new(driver_service, registration).run())
                                .detach();
                        }
                        Err(error) => println!("global shortcuts portal unavailable: {error}"),
                    }
                } else {
                    match X11rbKeyGrabber::connect() {
                        Ok(grabber) => {
                            let registration = X11ShortcutRegistration::new(grabber);
                            smol::spawn(ShortcutDriver::new(driver_service, registration).run())
                                .detach();
                        }
                        Err(error) => println!("cannot grab keys: {error}"),
                    }
                }
                // Let the registration settle before printing the record.
                smol::Timer::at(std::time::Instant::now() + Duration::from_millis(500)).await;
            }

            let capability = service.capability().await;
            println!("capability record: {capability:#?}");

            if let Some(shortcuts) = &shortcuts {
                let mut activations = shortcuts.activations();
                println!(
                    "waiting for {} ... (Ctrl-C to stop; shortcut state {:?}, error {:?})",
                    shortcuts.preferences().combination,
                    shortcuts.capability_state(),
                    shortcuts.registration_error()
                );
                use futures::StreamExt as _;
                while activations.next().await.is_some() {
                    println!("shortcut activated");
                    if let Some(result) = service.capture_on_activation().await {
                        report(result);
                    }
                }
                return;
            }

            if !options.delay.is_zero() {
                println!("capturing in {} s ...", options.delay.as_secs());
                smol::Timer::at(std::time::Instant::now() + options.delay).await;
            }
            let target: CaptureTarget = capability.target;
            println!("requesting a window capture, target {target:?}");
            report(service.capture(target).await);
        });
    }
}
