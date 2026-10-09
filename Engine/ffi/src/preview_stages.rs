//! PreviewStages keeps the preview view's render stages resident between renders.

use std::sync::Arc;

use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::pipeline::StageCache;
use lightcraft_engine::{RenderJob, SourceLevel};

/// PreviewStages owns the one StageCache of the session's preview view, the same per-view cache
/// upstream's frontend gives its loupe. Through it the engine keeps on the GPU (GpuStages, the
/// cache's extension) the uploaded source and the sampled, white-balanced and log-luminance
/// images, so a tone or exposure drag reruns only the per-pixel kernel instead of re-uploading
/// tens of MB per render.
///
/// The engine keys every stage by the source buffer it came from, so a stale stage can never be
/// reused for another photo: what this type adds is memory hygiene. It drops the cache when the
/// preview moves to another photo or source level (the old stages pin the old source and hold
/// 171 MB of GPU buffers on the NEF at 2360 px, 153 MB on the RAF, measured on the Mac at the
/// iPad budget), when the budget shrinks below what the stages hold, and (in session.rs) on
/// uv_suspend. Dropping means replacing the Arc: a render in flight keeps the old cache
/// through its own job and frees it when it ends, so it never repopulates the new one.
///
/// It lives on the engine thread; the render worker only sees the Arc clone attached to a job,
/// and the scheduler runs one job at a time, so the cache (internally locked anyway) is never
/// used by two renders at once.
#[derive(Default)]
pub(crate) struct PreviewStages {
    cache: Arc<StageCache>,
    owner: Option<(PhotoId, SourceLevel)>,
}

impl PreviewStages {
    /// attach gives job the preview cache, starting a fresh one when the job is for another photo
    /// or source level than the stages held.
    pub fn attach(&mut self, job: RenderJob) -> RenderJob {
        let owner = Some((job.photo, job.level));
        if self.owner != owner {
            self.cache = Arc::default();
            self.owner = owner;
        }
        job.with_stages(self.cache.clone())
    }

    /// keep_only drops the stages when they belong to a photo other than `active`, so closing or
    /// switching photos frees them without waiting for the next preview.
    pub fn keep_only(&mut self, active: Option<PhotoId>) {
        if self.owner.is_some_and(|(photo, _)| Some(photo) != active) {
            *self = PreviewStages::default();
        }
    }

    /// trim drops the stages when they hold more than a quarter of `budget` (CPU images plus GPU
    /// buffers). The quarter is Unveil's rule, not upstream's: upstream's frontend never trims its
    /// stage caches by budget. It is the same share the engine gives the decodes in flight.
    pub fn trim(&mut self, budget: usize) {
        if self.bytes() > budget / 4 {
            self.cache = Arc::default();
        }
    }

    fn bytes(&self) -> usize {
        self.cache.bytes() + lightcraft_engine::gpu::stage_bytes(&self.cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_engine::Session;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// Library is an engine Session over a fresh library in a temp folder, removed on drop.
    struct Library {
        session: Session,
        dir: PathBuf,
    }

    impl Library {
        fn new(name: &str) -> Library {
            let dir = std::env::temp_dir().join(format!("unveil-stages-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut session = Session::new().with_fs();
            session.open_library(&dir, false).unwrap();
            session.xmp.auto_write = false;
            Library { session, dir }
        }

        /// import adds a width x height gradient PNG and makes it the active photo.
        fn import(&mut self, width: u32, height: u32) -> PhotoId {
            let path = gradient_png(&self.dir, width, height);
            let report = self.exec("library.import", json!({"paths": [path], "mode": "add"}));
            let id = report["imported"][0].as_u64().expect("imported");
            self.exec("library.select", json!({"ids": [id], "active": id}));
            PhotoId(id)
        }

        fn exec(&mut self, command: &str, params: serde_json::Value) -> serde_json::Value {
            self.session.execute(command, &params).unwrap()
        }

        fn set_exposure(&mut self, value: f64) {
            self.exec(
                "develop.set",
                json!({"control": "light.exposure", "value": value}),
            );
        }

        /// render runs the active photo's preview job, with the preview stages when given, and
        /// accepts the result as the engine thread does, so the source stays cached between renders.
        fn render(&mut self, stages: Option<&mut PreviewStages>) -> Vec<[u8; 4]> {
            let id = self.session.active().unwrap();
            let mut job = self.session.render_job(id, 2048, 2048, false, true).unwrap();
            if let Some(stages) = stages {
                job = stages.attach(job);
            }
            let result = job.run();
            self.session.accept(&result);
            result.rendered.unwrap().image.data
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn gradient_png(dir: &Path, width: u32, height: u32) -> PathBuf {
        let path = dir.join(format!("gradient-{width}x{height}.png"));
        let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), width, height);
        encoder.set_color(png::ColorType::Rgb);
        let data: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| [(x * 2) as u8, (y * 3) as u8, 128]))
            .collect();
        encoder.write_header().unwrap().write_image_data(&data).unwrap();
        path
    }

    #[test]
    fn renders_with_resident_stages_match_renders_without() {
        let mut library = Library::new("exposure");
        library.import(640, 427);
        let mut stages = PreviewStages::default();

        library.set_exposure(-0.7);
        let first = library.render(Some(&mut stages));
        assert!(stages.bytes() > 0, "the first render left no stage behind");
        let first_plain = library.render(None);

        library.set_exposure(1.3);
        let second = library.render(Some(&mut stages));
        let second_plain = library.render(None);

        assert!(first == first_plain, "exposure -0.7 differs with stages");
        assert!(
            second == second_plain,
            "exposure 1.3 differs with resident stages"
        );
        assert!(first != second, "the exposure change had no effect");
    }

    /// stages_follow_white_balance_geometry_and_noise_reduction edits what the cached stages hold
    /// (the white-balanced image, the sampled geometry, the denoised planes) between cached renders,
    /// so a stage keyed too loosely shows up as a frame that differs from a render without stages.
    #[test]
    fn stages_follow_white_balance_geometry_and_noise_reduction() {
        let mut library = Library::new("keys");
        library.import(640, 427);
        let mut stages = PreviewStages::default();
        let mut previous = library.render(Some(&mut stages));
        let edits = [
            ("wb.temp", 4200.0),
            ("wb.tint", 25.0),
            ("crop.angle", 3.5),
            ("detail.nrLuminance", 60.0),
            ("detail.nrColor", 60.0),
        ];
        for (control, value) in edits {
            library.exec("develop.set", json!({"control": control, "value": value}));
            let cached = library.render(Some(&mut stages));
            assert!(cached == library.render(None), "{control} reused a stale stage");
            assert!(cached != previous, "{control} had no effect");
            previous = cached;
        }
    }

    #[test]
    fn switching_photos_never_reuses_the_previous_photos_stages() {
        let mut library = Library::new("switch");
        let first = library.import(640, 427);
        let mut stages = PreviewStages::default();
        library.render(Some(&mut stages));
        let first_cache = stages.cache.clone();

        let second = library.import(480, 320);
        stages.keep_only(Some(second));
        assert!(
            stages.owner.is_none() && stages.bytes() == 0,
            "the first photo's stages survived"
        );
        let with_stages = library.render(Some(&mut stages));
        assert!(
            !Arc::ptr_eq(&stages.cache, &first_cache),
            "the second photo got the first's cache"
        );
        assert_eq!(stages.owner.map(|(photo, _)| photo), Some(second));
        assert!(
            with_stages == library.render(None),
            "the second photo differs with stages"
        );

        // attach alone, without keep_only, must also start over for another photo
        library.exec("library.select", json!({"ids": [first.0], "active": first.0}));
        let second_cache = stages.cache.clone();
        library.render(Some(&mut stages));
        assert!(
            !Arc::ptr_eq(&stages.cache, &second_cache),
            "the first photo got the second's cache"
        );
    }

    #[test]
    fn a_small_budget_drops_the_stages() {
        let mut library = Library::new("trim");
        library.import(640, 427);
        let mut stages = PreviewStages::default();
        library.render(Some(&mut stages));
        let held = stages.bytes();
        assert!(held > 0);

        stages.trim(held * 4);
        assert_eq!(stages.bytes(), held, "a budget that fits the stages dropped them");
        stages.trim(held * 4 - 4);
        assert_eq!(stages.bytes(), 0, "a budget too small kept the stages");
    }
}
