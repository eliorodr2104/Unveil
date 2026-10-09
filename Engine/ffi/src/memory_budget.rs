//! Memory policy of the host: the page-release hook the engine calls after dropping large buffers.

unsafe extern "C" {
    // libSystem on iOS and macOS: returns freed pages to the system after a cache trim.
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

fn release_memory() {
    // SAFETY: a null zone means "all zones"; goal 0 means "as much as possible".
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}

/// install_release_hook registers release_memory with the engine. The engine keeps the first hook
/// installed, so calling this for every session is harmless.
pub(crate) fn install_release_hook() {
    lightcraft_engine::memory::set_release_hook(release_memory);
}
