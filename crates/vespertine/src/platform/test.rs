mod chrome;
mod composition;
mod dispatcher;
mod display;
mod lifecycle;
mod platform;
mod window;

#[cfg(test)]
mod embedded_launch_tests;

pub(crate) use chrome::*;
pub(crate) use composition::*;
pub use dispatcher::*;
pub(crate) use display::*;
pub(crate) use lifecycle::*;
pub(crate) use platform::*;
pub(crate) use window::*;

#[cfg(any(test, feature = "test-support"))]
pub use lifecycle::TestActivation;
#[cfg(any(test, feature = "test-support"))]
pub use platform::{TestScreenCaptureSource, TestScreenCaptureStream};
