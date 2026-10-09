//! The unveil-render thread runs RenderJob::run, so the engine thread never waits on pixels.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use lightcraft_engine::RenderJob;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::RenderResult;

use crate::session::{FrameCallback, Msg, caught, set_user_initiated_qos};

/// Work is one render for the worker: the job (its request_id is the generation) and what the
/// engine thread needs back to deliver the frame.
pub(crate) struct Work {
    pub job: RenderJob,
    pub draft: bool,
    pub callback: FrameCallback,
}

/// RenderDone is a finished Work, sent back to the engine thread. `result` is None when the
/// render panicked: the engine thread must still hear about it, or the scheduler would stall.
pub(crate) struct RenderDone {
    pub generation: u64,
    pub draft: bool,
    pub photo: PhotoId,
    pub result: Option<RenderResult>,
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
                for Work { job, draft, callback } in rx {
                    let (generation, photo) = (job.request_id, job.photo);
                    let result = caught("render", || job.run());
                    let done_msg = RenderDone {
                        generation,
                        draft,
                        photo,
                        result,
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
