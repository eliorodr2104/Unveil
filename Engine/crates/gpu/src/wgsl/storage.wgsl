// Modified by the Unveil authors, 2026: half storage with explicit f32 loads; see Engine/CHANGES.md.
// Storage conversion only. A rejected candidate is never installed in the stage cache.
@compute @workgroup_size(256)
fn pack_half(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) { return; }
    let v = src[i];
    if ((bitcast<u32>(v) & 0x7fffffffu) > 0x477fe000u) {
        atomicOr(&bad[0], 1u);
        dst[i] = f16(0.0);
    } else { dst[i] = f16(v); }
}
