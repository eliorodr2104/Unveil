//! unveil-ffi is the C ABI between the Unveil app and the LightCraft engine (include/uv.h).

// Reference the engine so the staticlib links it and native-static-libs lists its frameworks.
extern crate lightcraft_engine as _;

mod last_error;
mod memory_budget;
mod panic_guard;
mod preview_scheduler;
mod render_worker;
mod session;
mod status;

pub use last_error::uv_last_error;
pub use session::*;
pub use status::UVStatus;

/// uv_abi_version lets the Swift side refuse a framework built for another header.
/// It returns a constant and cannot fail, so it skips the panic guard.
#[unsafe(no_mangle)]
pub extern "C" fn uv_abi_version() -> u32 {
    1
}
