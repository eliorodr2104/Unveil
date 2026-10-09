# Unveil

iPad RAW editor (iPadOS 26+): UIKit app shell, SwiftUI panels, and a Rust engine in `Engine/` (the vendored LightCraft workspace plus the `Engine/ffi` bridge).

## Read first

- Spec: `Docs/Specs/2026-10-09-v0-motore-su-ipad-design.md`
- Plan: `Docs/Plans/2026-10-09-v0-motore-su-ipad.md`
- Style: `CODE_STYLE.md` (comments in English, no em-dashes, `//` at most 2 lines)

## Commands

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"    # before any cargo command
(cd Engine && cargo test -p unveil-ffi --features test-hooks)
scripts/build-xcframework.sh                        # created by a later ticket
scripts/test-app.sh                                 # created by a later ticket
```

## Rules

- `Engine/crates` is upstream code: do not modify it in v0.
- `lightcraft-engine` is used without features; never call `with_default_face_models` or `with_default_denoise_models`.
- Commits are made only by the coordinator: workers never stage, commit or push.
