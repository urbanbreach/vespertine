//! Prints the desktop capture, accessibility and microphone permission status
//! of this Linux desktop (PLT-013 R5). Querying never prompts and never writes.
//! Pass `--request-accessibility` or `--request-microphone` to run that
//! domain's explicit request hook.

use std::sync::Arc;
use vespertine_desktop::permissions::{
    PermissionDomain, PermissionService, linux::PortalCaptureStatusSource,
};

fn main() {
    let service = PermissionService::linux(Arc::new(PortalCaptureStatusSource));
    smol::block_on(async {
        for domain in PermissionDomain::ALL {
            println!("{domain:?}: {:?}", service.status(domain).await);
        }
        for argument in std::env::args().skip(1) {
            let domain = match argument.as_str() {
                "--request-accessibility" => PermissionDomain::Accessibility,
                "--request-microphone" => PermissionDomain::Microphone,
                "--request-capture" => PermissionDomain::DesktopCapture,
                other => {
                    eprintln!("unknown argument {other}");
                    continue;
                }
            };
            let result = service.request(domain).await;
            println!(
                "request {domain:?}: {:?} -> {:?}",
                result.outcome, result.status
            );
        }
    });
}
