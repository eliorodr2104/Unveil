# iOS build baseline (T2)

First build of the engine for iPadOS behind the empty `unveil-ffi` crate (`uv_abi_version() -> 1`).
Toolchain: rustup stable with `aarch64-apple-ios` and `aarch64-apple-ios-sim` added.
All builds: `IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo build -p unveil-ffi --release [--target T]` from `Engine/`.

## Commands

Run from `Engine/` with `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`.

Step 1 (targets):
```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

Step 3 (dependency tree):
```bash
cargo tree -p unveil-ffi --target aarch64-apple-ios -e normal,build --prefix none \
  | sort -u | grep -E '^(ring|ash|cc|libfuzzer-sys|rav1e|built|harfrust|objc2-metal|wgpu) ' | tee /tmp/deps.txt
```

Step 4 (builds, one per platform):
```bash
export IPHONEOS_DEPLOYMENT_TARGET=26.0
cargo build -p unveil-ffi --release --target aarch64-apple-ios
cargo build -p unveil-ffi --release --target aarch64-apple-ios-sim
cargo build -p unveil-ffi --release
ls -la target/aarch64-apple-ios/release/libunveil_ffi.a target/aarch64-apple-ios-sim/release/libunveil_ffi.a
```

Step 5 (native libs, reused by T7):
```bash
cargo rustc -p unveil-ffi --release --target aarch64-apple-ios --crate-type staticlib -- --print native-static-libs 2>&1 \
  | grep 'native-static-libs' | tee /tmp/native-libs.txt
```

## Results

| Target | Result | Wall time | Build kind | `libunveil_ffi.a` |
|---|---|---|---|---|
| aarch64-apple-ios | OK, no fix needed | 2m29s | clean for the target (no prior iOS artifacts) | 407,877,008 bytes (389 MB) |
| aarch64-apple-ios-sim | OK | 2m33s | clean for the target; host build scripts and proc-macros reused from the ios build | 407,247,320 bytes (388 MB) |
| aarch64-apple-darwin (host) | OK | 1m54s | incremental: some host-side crates already built by the iOS builds; `Engine/target` was not wiped first | 407,351,648 bytes (388 MB) |

Sizes are of the release staticlib with debuginfo (the workspace release profile keeps it); the linked app binary is much smaller.

## Dependency tree findings (`cargo tree --target aarch64-apple-ios -e normal,build`)

Present: `built 0.8.1`, `harfrust 0.12.0`, `objc2-metal 0.3.2`, `rav1e 0.8.1`, `wgpu 30.0.1`.
Absent: `ring`, `ash` (no Vulkan), `cc`, `libfuzzer-sys`.
TLS goes through `rustls-rustcrypto`, which compiled for iOS.

## native-static-libs (input for T7)

```
-lSystem -framework QuartzCore -framework CoreGraphics -framework Metal -framework Foundation -framework CoreFoundation -lobjc -framework Foundation -liconv -lSystem -lc -lm
```

This only lists the frameworks because `lib.rs` has `extern crate lightcraft_engine as _;`. Without a reference, rustc does not link the engine.

## Warnings

- `lightcraft-engine@0.4.0: no craft-fonts embedded: Chinese and Japanese text will render as empty boxes.` (build.rs warning, all targets; CJK text rendering is not needed in v0.)
- No rustc warnings from `unveil-ffi`.
