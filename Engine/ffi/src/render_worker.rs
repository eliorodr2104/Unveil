//! The unveil-render thread runs RenderJob::run, so the engine thread never waits on pixels.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use lightcraft_engine::RenderJob;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::RenderResult;

use crate::diagnostics::{Category, Part, Span, record};
use crate::session::{Delivery, FrameCallback, Msg, caught, set_user_initiated_qos};

/// Work is one render for the worker: the job (its request_id is the generation), the small whole
/// frame whose histogram a window carries, if wanted, and what the engine thread needs back to
/// deliver the frame.
pub(crate) struct Work {
    pub job: RenderJob,
    pub histogram_job: Option<RenderJob>,
    pub draft: bool,
    pub delivery: Delivery,
    pub callback: FrameCallback,
}

/// RenderDone is a finished Work, sent back to the engine thread. `result` is None when the
/// render panicked: the engine thread must still hear about it, or the scheduler would stall.
/// `histogram` is the histogram job's result, None when there was none or it panicked.
pub(crate) struct RenderDone {
    pub generation: u64,
    pub draft: bool,
    pub photo: PhotoId,
    pub result: Option<RenderResult<lightcraft_gpu::DisplayImage>>,
    pub histogram: Option<RenderResult>,
    pub delivery: Delivery,
    pub callback: FrameCallback,
}

/// RenderWorker is the engine thread's handle on unveil-render. Dropping `tx` ends the thread.
pub(crate) struct RenderWorker {
    tx: Sender<Work>,
    thread: JoinHandle<()>,
}

impl RenderWorker {
    /// spawn starts unveil-render, which answers every Work with a Msg::RenderDone on `done`.
    pub fn spawn(done: Sender<Msg>) -> std::io::Result<RenderWorker> {
        let (tx, rx) = mpsc::channel::<Work>();
        let thread = thread::Builder::new()
            .name("unveil-render".into())
            .stack_size(8 << 20)
            .spawn(move || {
                set_user_initiated_qos();
                for Work {
                    job,
                    histogram_job,
                    draft,
                    delivery,
                    callback,
                } in rx
                {
                    let (generation, photo) = (job.request_id, job.photo);
                    let category = Category::of(draft, delivery.placement.is_some());
                    let span = Span::start();
                    // Keep eligible GPU pixels mapped through the engine-thread frame callback.
                    let result = caught("render", || job.run_for_display(true));
                    record(category, Part::Render, &span);
                    if let Some(Ok(rendered)) = result.as_ref().map(|r| &r.rendered) {
                        crate::diagnostics::record_output(
                            category,
                            rendered.image.is_mapped(),
                            (rendered.image.width, rendered.image.height),
                            rendered.display_size,
                        );
                    }
                    let span = Span::start();
                    let histogram = histogram_job.and_then(|job| caught("histogram render", || job.run()));
                    if histogram.is_some() {
                        record(category, Part::Histogram, &span);
                    }
                    let done_msg = RenderDone {
                        generation,
                        draft,
                        photo,
                        result,
                        histogram,
                        delivery,
                        callback,
                    };
                    // The engine thread is gone only after a shutdown that gave up on this render.
                    if done.send(Msg::RenderDone(Box::new(done_msg))).is_err() {
                        return;
                    }
                }
            })?;
        Ok(RenderWorker { tx, thread })
    }

    /// run queues one render; false means the worker is gone and no RenderDone will come.
    pub fn run(&self, work: Work) -> bool {
        self.tx.send(work).is_ok()
    }

    /// stop closes the queue. With `idle` the thread is about to exit and is joined; otherwise a
    /// render is stuck and the thread is left to finish on its own.
    pub fn stop(self, idle: bool) {
        drop(self.tx);
        if idle {
            let _ = self.thread.join();
        }
        // ponytail: a stuck render keeps its detached thread and memory until it returns; cancel
        // it when upstream grows a cancellation hook.
    }
}
