//! Desktop integration services that do not depend on a window system:
//! feedback sounds (PLT-011), permission-aware capture and global shortcuts
//! (PLT-012), bounded accessibility context and device permission status
//! (PLT-013).

pub mod capture;
pub mod capture_glue;
pub mod context;
pub mod permissions;
pub mod shortcut;
pub mod sound;
