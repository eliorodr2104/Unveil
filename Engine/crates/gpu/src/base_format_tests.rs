//! Bit-exact storage loads and scoped optional-compiler failure on a real device.
use super::*;

static PROBE: Module = Module {
    src: r#"
@compute @workgroup_size(64)
fn storage_probe(@builtin(global_invocation_id) g: vec3<u32>) {
let i = g.x;
if (i >= P[0]) { return; }
out[i*6u+0u] = bitcast<u32>(load_a(i));
out[i*6u+1u] = bitcast<u32>(load_b(i));
out[i*6u+2u] = bitcast<u32>(load_c(i));
out[i*6u+3u] = bitcast<u32>(load_d(i));
out[i*6u+4u] = bitcast<u32>(load_e(i));
out[i*6u+5u] = bitcast<u32>(load_f(i));
}
"#,
    bindings: &[
        ("a", false, "float_storage"),
        ("b", false, "float_storage"),
        ("c", false, "float_storage"),
        ("d", false, "float_storage"),
        ("e", false, "float_storage"),
        ("f", false, "float_storage"),
        ("out", true, "u32"),
    ],
    entries: &["storage_probe"],
};

fn install(g: &mut Gpu, loads: StorageLoads) {
    let (layout, mut entries) = compile_module(&g.device, &PROBE, &constants(), loads, false).unwrap();
    g.kernels.insert("storage_probe", Kernel { pipeline: entries.pop().unwrap(), layout, nbuf: 7 });
}

// Exact finite half conversion, independent of the GPU pack shader and its rounding.
fn expand(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (h >> 10) & 31;
    let fraction = (h & 1023) as f32;
    sign * if exponent == 0 { fraction * 2.0f32.powi(-24) } else { (1.0 + fraction / 1024.0) * 2.0f32.powi(exponent as i32 - 15) }
}

fn run_probe(g: &Gpu, mask: u32) -> Vec<u32> {
    let half = [0x0000u16, 0x8000, 0x0001, 0x8001, 0x03ff, 0x0400, 0x3c00, 0xbc00, 0x7bff, 0xfbff, 0x3555];
    let n = half.len();
    let mut buffers = Vec::new();
    let mut expected = vec![0; n * 6];
    for b in 0..6 {
        if mask & (1 << b) != 0 {
            let mut bits: Vec<_> = (0..n).map(|i| half[(i + b) % n]).collect();
            for i in 0..n {
                expected[i * 6 + b] = expand(bits[i]).to_bits();
            }
            bits.push(0x7e00); // Poison the one padded half; it must not become a logical sample.
            let words: Vec<_> = bits.chunks_exact(2).map(|v| v[0] as u32 | ((v[1] as u32) << 16)).collect();
            let mut buffer = g.upload(&words);
            buffer.half_len = Some(n);
            buffers.push(buffer);
        } else {
            let bits = [0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x00800000, 0x3f800000, 0xbf800000, 0x7f7fffff, 0xff7fffff, 0x3eaaaaab];
            let values: Vec<_> = (0..n).map(|i| f32::from_bits(bits[(i + b) % n])).collect();
            for i in 0..n {
                expected[i * 6 + b] = values[i].to_bits();
            }
            buffers.push(g.upload(&values));
        }
    }
    let out = g.upload(&vec![0xdeadbeefu32; n * 6 + 1]);
    let mut bindings: Vec<_> = buffers.iter().map(Some).collect();
    bindings.push(Some(&out));
    let scopes = ErrorScopes::push(g);
    let mut enc = g.encoder();
    g.run(&mut enc, "storage_probe", &[n as u32], &bindings, [1, 1, 1]);
    let result: Vec<u32> = g.finish_and_read(enc, &out, n * 6 + 1);
    assert!(scopes.pop().is_none(), "optional failure escaped its scopes");
    assert!(!failed(), "render failed");
    assert_eq!(&result[..n * 6], &expected, "mask={mask}");
    assert_eq!(result[n * 6], 0xdeadbeef, "tail was overwritten");
    result
}

#[test]
fn direct_f32_loads_match_dynamic_bits_and_survive_optional_compile_failure() {
    let mut gpu = Gpu::new(wgpu::Backends::PRIMARY).expect("real GPU required");
    install(&mut gpu, StorageLoads::Dynamic);
    let expected = run_probe(&gpu, 0);
    install(&mut gpu, StorageLoads::F32);
    assert_eq!(run_probe(&gpu, 0), expected);
    assert!(compile_module(&gpu.device, &PROBE, &constants(), StorageLoads::F32, true).is_err());
    install(&mut gpu, StorageLoads::Dynamic);
    assert_eq!(run_probe(&gpu, 0), expected, "scoped compiler failure must leave dynamic fallback usable");
    assert!(!failed());
}

#[test]
fn dynamic_loads_preserve_every_mixed_mask_and_ignore_odd_half_padding() {
    let mut gpu = Gpu::new(wgpu::Backends::PRIMARY).expect("real GPU required");
    install(&mut gpu, StorageLoads::Dynamic);
    for mask in 0..64 {
        run_probe(&gpu, mask);
    }
}
