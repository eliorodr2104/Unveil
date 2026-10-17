// Modified by the Unveil authors, 2026: area reduction after encoded RGBA8 color; see Engine/CHANGES.md.
// P: native width/height, crop x/y/width/height, display width/height, native count.
// Each workgroup accumulates locally; only two global atomic updates leave it.
var<workgroup> alpha_missing: atomic<u32>;
var<workgroup> alpha_visited: atomic<u32>;

@compute @workgroup_size(256)
fn display_validate(@builtin(local_invocation_index) thread: u32, @builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    if (thread == 0u) {
        atomicStore(&alpha_missing, 0u);
        atomicStore(&alpha_visited, 0u);
    }
    workgroupBarrier();
    var missing = 0u;
    var visited = 0u;
    for (var i = group.x * 256u + thread; i < pu(8u); i += groups.x * 256u) {
        missing += select(0u, 1u, (src[i] >> 24u) != 255u);
        visited += 1u;
    }
    atomicAdd(&alpha_missing, missing);
    atomicAdd(&alpha_visited, visited);
    workgroupBarrier();
    if (thread == 0u) {
        atomicAdd(&bad[0u], atomicLoad(&alpha_missing));
        atomicAdd(&bad[1u], atomicLoad(&alpha_visited));
    }
}

fn reduce_pixel(g: vec3<u32>, check_alpha: bool) -> u32 {
    let dw = pu(6u);
    let dh = pu(7u);
    let sw = pu(4u);
    let sh = pu(5u);
    let x0 = g.x * sw;
    let x1 = (g.x + 1u) * sw;
    let y0 = g.y * sh;
    let y1 = (g.y + 1u) * sh;
    var rgb = vec3<f32>(0.0);
    var complete = true;
    for (var sy = y0 / dh; sy < (y1 + dh - 1u) / dh; sy++) {
        let wy = f32(min(y1, (sy + 1u) * dh) - max(y0, sy * dh)) / f32(sh);
        for (var sx = x0 / dw; sx < (x1 + dw - 1u) / dw; sx++) {
            let wx = f32(min(x1, (sx + 1u) * dw) - max(x0, sx * dw)) / f32(sw);
            let px = src[(pu(3u) + sy) * pu(0u) + pu(2u) + sx];
            if (check_alpha) { complete = complete && (px >> 24u) == 255u; }
            rgb += vec3<f32>(f32(px & 255u), f32((px >> 8u) & 255u), f32((px >> 16u) & 255u)) * wx * wy;
        }
    }
    let q = vec3<u32>(floor(clamp(rgb, vec3<f32>(0.0), vec3<f32>(255.0)) + vec3<f32>(0.5)));
    return q.x | (q.y << 8u) | (q.z << 16u) | (select(0u, 255u, complete) << 24u);
}

@compute @workgroup_size(16, 16)
fn display_reduce(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(6u) || g.y >= pu(7u)) { return; }
    dst[g.y * pu(6u) + g.x] = reduce_pixel(g, false);
}

// Every visible native pixel contributes positive area to at least one output pixel.
// Invalid native alpha propagates to output; omitted output work remains zero from clear.
@compute @workgroup_size(16, 16)
fn display_reduce_checked(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(6u) || g.y >= pu(7u)) { return; }
    dst[g.y * pu(6u) + g.x] = reduce_pixel(g, true);
}

struct HalfAxis {
    first: u32,
    count: u32,
    weights: vec3<f32>,
}

// n is either 2*d or 2*d-1. These are the original integer overlap numerators;
// weights are divided in f32 exactly as in reduce_pixel, before the same row-major sum.
fn half_axis(i: u32, n: u32, d: u32) -> HalfAxis {
    if (n == 2u * d) {
        let half = f32(d) / f32(n);
        return HalfAxis(2u * i, 2u, vec3<f32>(half, half, 0.0));
    }
    let middle = f32(d) / f32(n);
    let right = f32(d - i - 1u) / f32(n);
    if (i == 0u) {
        // n=1 has only the middle sample, with weight exactly one.
        return HalfAxis(0u, min(2u, n), vec3<f32>(middle, right, 0.0));
    }
    let left = f32(i) / f32(n);
    let count = select(3u, 2u, i + 1u == d);
    return HalfAxis(2u * i - 1u, count, vec3<f32>(left, middle, right));
}

fn reduce_pixel_fast(g: vec3<u32>, check_alpha: bool) -> u32 {
    let ax = half_axis(g.x, pu(4u), pu(6u));
    let ay = half_axis(g.y, pu(5u), pu(7u));
    var rgb = vec3<f32>(0.0);
    var complete = true;
    for (var iy = 0u; iy < ay.count; iy++) {
        let sy = ay.first + iy;
        let wy = ay.weights[iy];
        for (var ix = 0u; ix < ax.count; ix++) {
            let sx = ax.first + ix;
            let wx = ax.weights[ix];
            let px = src[(pu(3u) + sy) * pu(0u) + pu(2u) + sx];
            if (check_alpha) { complete = complete && (px >> 24u) == 255u; }
            rgb += vec3<f32>(f32(px & 255u), f32((px >> 8u) & 255u), f32((px >> 16u) & 255u)) * wx * wy;
        }
    }
    let q = vec3<u32>(floor(clamp(rgb, vec3<f32>(0.0), vec3<f32>(255.0)) + vec3<f32>(0.5)));
    return q.x | (q.y << 8u) | (q.z << 16u) | (select(0u, 255u, complete) << 24u);
}

@compute @workgroup_size(16, 16)
fn display_reduce_fast(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(6u) || g.y >= pu(7u)) { return; }
    dst[g.y * pu(6u) + g.x] = reduce_pixel_fast(g, false);
}

@compute @workgroup_size(16, 16)
fn display_reduce_checked_fast(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(6u) || g.y >= pu(7u)) { return; }
    dst[g.y * pu(6u) + g.x] = reduce_pixel_fast(g, true);
}

// Reduced-only statistics share this output dispatch. P[9] is the unchanged CPU
// histogram sampling step, relative to the already cropped physical display image.
var<workgroup> histogram_bins: array<atomic<u32>, 1024>;

@compute @workgroup_size(16, 16)
fn display_reduce_checked_stats(
    @builtin(local_invocation_id) local: vec3<u32>,
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) thread: u32,
) {
    for (var bin = thread; bin < 1024u; bin += 256u) {
        atomicStore(&histogram_bins[bin], 0u);
    }
    if (thread == 0u) {
        atomicStore(&alpha_missing, 0u);
        atomicStore(&alpha_visited, 0u);
    }
    workgroupBarrier();
    var missing = 0u;
    var visited = 0u;
    let tile = REDUCTION_TILE;
    for (var ty = 0u; ty < tile; ty++) {
        for (var tx = 0u; tx < tile; tx++) {
            let g = vec3<u32>(group.xy * (16u * tile) + local.xy + vec2<u32>(tx, ty) * 16u, 0u);
    if (g.x < pu(6u) && g.y < pu(7u)) {
        let pixel = reduce_pixel_fast(g, true);
        dst[g.y * pu(6u) + g.x] = pixel;
        missing += select(0u, 1u, (pixel >> 24u) != 255u);
        visited += 1u;
        if (g.x % pu(9u) == 0u && g.y % pu(9u) == 0u) {
            let r = pixel & 255u;
            let green = (pixel >> 8u) & 255u;
            let b = (pixel >> 16u) & 255u;
            let luma = (54u * r + 183u * green + 19u * b) >> 8u;
            atomicAdd(&histogram_bins[r], 1u);
            atomicAdd(&histogram_bins[256u + green], 1u);
            atomicAdd(&histogram_bins[512u + b], 1u);
            atomicAdd(&histogram_bins[768u + luma], 1u);
        }
    }
        }
    }
    atomicAdd(&alpha_missing, missing);
    atomicAdd(&alpha_visited, visited);
    workgroupBarrier();
    for (var bin = thread; bin < 1024u; bin += 256u) {
        let count = atomicLoad(&histogram_bins[bin]);
        if (count != 0u) { atomicAdd(&bad[bin], count); }
    }
    if (thread == 0u) {
        atomicAdd(&bad[1024u], atomicLoad(&alpha_missing));
        atomicAdd(&bad[1025u], atomicLoad(&alpha_visited));
    }
}
