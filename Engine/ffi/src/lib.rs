//! unveil-ffi is the C ABI between the Unveil app and the LightCraft engine.

// Reference the engine so the staticlib links it and native-static-libs lists its frameworks.
extern crate lightcraft_engine as _;

/// uv_abi_version lets the Swift side refuse a framework built for another header.
#[unsafe(no_mangle)]
pub extern "C" fn uv_abi_version() -> u32 {
    1
}
