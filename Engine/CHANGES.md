# Changes to the vendored engine

Files under `crates/` that differ from upstream (see `UPSTREAM.md` for the commit), as Apache-2.0
section 4(b) requires. Each changed file also carries a notice at its top.

| Date | File | What | Why |
|---|---|---|---|
| 2026-10-09 | `crates/gpu/src/ctx.rs` | `Gpu::upload` writes uploads under 1 MiB into a buffer from the free pool (`buffer(len)` plus `queue.write_buffer`) instead of always creating a new one. Larger uploads, usage flags and labels are unchanged. | Each preview uploads a 32 KiB LUT block. The new buffer joined the free pool on drop and was never reused, so the pool grew by one buffer per frame up to its limit (about 317 MiB and +3 MiB/s of footprint on the iPad once resident stages removed the per-frame source upload). |
