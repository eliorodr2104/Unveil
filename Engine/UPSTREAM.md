# Upstream

Source: https://github.com/storytold/lightcraft
Commit: c435d143de921e8dc724245065500bc191f91102 (2026-10-09)
Used under the Apache License 2.0 option of upstream's "MIT OR Apache-2.0" dual license.

Copied: crates/ (without ui-egui and mcp), assets/, Cargo.toml, Cargo.lock, rustfmt.toml, clippy.toml.
Not copied: .gitignore, apps/, xtask/, docs/ (including the ArtCraft trademarks in docs/brand/), tools/, packaging/, nix/, contributors/, .cargo/, .github/, README.md, AGENTS.md, CLAUDE.md, ROADMAP.md, LICENSE-MIT, NOTICE (folded into the root NOTICE), flake.nix, flake.lock.
Local changes in v0: Engine/Cargo.toml `members` (now `["crates/*"]`), and Engine/Cargo.lock pruned
by Cargo (packages of the excluded crates removed, no version changed). Every later change to a file
under crates/ is listed in CHANGES.md next to this file, as Apache-2.0 section 4(b) requires.
