//! stage_bench <raw> [--cpu]: 60 previews at 2360 px of one exposure drag, without and with the
//! preview stage cache, as wall and render-thread CPU time per render (median and p95).
//! `--cpu` switches the GPU off, to time the CPU pipeline. Build it with --release.
use std::sync::Arc;
use std::time::Instant;

use lightcraft_engine::Session;
use lightcraft_engine::pipeline::StageCache;
use serde_json::json;

const RENDERS: usize = 60;
const MAX_PIXELS: usize = 2360;
const CLOCK_THREAD_CPUTIME_ID: u32 = 16;

unsafe extern "C" {
    fn clock_gettime_nsec_np(clock_id: u32) -> u64;
}

fn thread_cpu_ms() -> f64 {
    // SAFETY: plain libc call with a valid clock id; it takes no pointers.
    unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) as f64 / 1e6 }
}

/// drag renders RENDERS previews, each after an exposure step, and returns (wall, cpu) ms per
/// render. One untimed render first loads the source into the engine cache, as an open would.
fn drag(session: &mut Session, stages: Option<&Arc<StageCache>>) -> (Vec<f64>, Vec<f64>) {
    let id = session.active().expect("an active photo");
    let mut times = (Vec::new(), Vec::new());
    for i in 0..=RENDERS {
        let exposure = -1.5 + i as f64 * 0.05;
        let params = json!({"control": "light.exposure", "value": exposure});
        session.execute("develop.set", &params).unwrap();
        let mut job = session
            .render_job(id, MAX_PIXELS, MAX_PIXELS, false, true)
            .unwrap();
        if let Some(stages) = stages {
            job = job.with_stages(stages.clone());
        }
        let (wall, cpu) = (Instant::now(), thread_cpu_ms());
        let result = job.run();
        let (wall, cpu) = (wall.elapsed().as_secs_f64() * 1e3, thread_cpu_ms() - cpu);
        result.rendered.as_ref().expect("a rendered preview");
        session.accept(&result);
        if i > 0 {
            times.0.push(wall);
            times.1.push(cpu);
        }
    }
    times
}

fn stats(mut ms: Vec<f64>) -> String {
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    format!("median {:6.2} ms  p95 {:6.2} ms", at(0.5), at(0.95))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let raw = args.next().expect("usage: stage_bench <raw> [--cpu]");
    if args.next().as_deref() == Some("--cpu") {
        lightcraft_engine::gpu::set_enabled(false);
    }
    let dir = std::env::temp_dir().join(format!("unveil-stage-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single-threaded here, before the engine starts any thread; same rule as uv_session_new.
    unsafe { std::env::set_var("LIGHTCRAFT_CAMERA_PROFILES", dir.join("camera-profiles")) };
    let mut session = Session::new().with_fs();
    session.open_library(&dir, false).unwrap();
    session.xmp.auto_write = false;
    let report = session
        .execute("library.import", &json!({"paths": [raw], "mode": "add"}))
        .unwrap();
    let id = report["imported"][0].as_u64().expect("imported");
    session
        .execute("library.select", &json!({"ids": [id], "active": id}))
        .unwrap();

    println!(
        "{} renders at {MAX_PIXELS} px, gpu {}",
        RENDERS,
        lightcraft_engine::gpu::enabled()
    );
    for (name, stages) in [("without stages", None), ("with stages", Some(Arc::default()))] {
        let (wall, cpu) = drag(&mut session, stages.as_ref());
        println!("{name:15} wall {}   cpu {}", stats(wall), stats(cpu));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
