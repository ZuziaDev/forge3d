//! Single-writer document hosting.
//!
//! The [`Document`] is owned by exactly one thread. Everybody else (MCP
//! sessions, the CLI script runner, background tasks) holds a cheap,
//! cloneable [`DocumentHandle`] and submits *jobs* — closures that receive
//! `&mut Document` — over a channel. The owner runs the jobs one after the
//! other, so every operation sees a consistent document and no locks are
//! needed around the scene.
//!
//! Two hosting modes:
//!
//! * **headless** – [`DocumentHost::spawn`] starts a dedicated owner thread
//!   that blocks on the channel and runs jobs as they arrive;
//! * **embedded** – [`DocumentHandle::channel`] returns the handle plus a
//!   [`DocumentJobs`] receiver. The GUI main thread owns the document and
//!   calls [`DocumentJobs::drain`] once per frame; the optional waker passed
//!   to [`DocumentHandle::channel_with_waker`] is invoked whenever a job is
//!   queued, so the GUI can request a redraw and AI edits appear live.
//!
//! ```
//! use forge3d_api::host::{DocumentHandle, DocumentHost};
//! use forge3d_core::prelude::*;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let handle = DocumentHost::spawn(Document::new());
//! let objects = handle.run(|doc| doc.scene.len()).await.unwrap();
//! assert_eq!(objects, 0);
//!
//! // Embedded: the caller owns the document and drains jobs itself.
//! let (handle, jobs) = DocumentHandle::channel();
//! let mut doc = Document::new();
//! let pending = tokio::spawn({
//!     let handle = handle.clone();
//!     async move { handle.run(|doc| doc.revision()).await }
//! });
//! while !pending.is_finished() {
//!     jobs.drain(&mut doc);
//!     tokio::task::yield_now().await;
//! }
//! assert_eq!(pending.await.unwrap().unwrap(), 0);
//! # });
//! ```

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use forge3d_core::document::Document;

use crate::error::{ApiError, ApiResult};

/// A unit of work executed on the document owner's thread.
pub type DocumentJob = Box<dyn FnOnce(&mut Document) + Send>;

/// Callback invoked whenever a job is queued (embedded mode).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Cheaply cloneable, `Send + Sync` handle that submits jobs to the thread
/// owning the [`Document`].
#[derive(Clone)]
pub struct DocumentHandle {
    tx: Sender<DocumentJob>,
    waker: Option<Waker>,
}

impl std::fmt::Debug for DocumentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentHandle")
            .field("has_waker", &self.waker.is_some())
            .finish()
    }
}

/// Receiving side of an embedded document channel. The owner of the
/// document calls [`DocumentJobs::drain`] regularly (e.g. once per frame).
pub struct DocumentJobs {
    rx: Receiver<DocumentJob>,
}

impl std::fmt::Debug for DocumentJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentJobs")
            .field("pending", &self.rx.len())
            .finish()
    }
}

fn run_job(job: DocumentJob, doc: &mut Document) {
    // A panicking job must not take the document owner down with it; the
    // caller's reply channel is dropped and it receives `internal_error`.
    if catch_unwind(AssertUnwindSafe(|| job(doc))).is_err() {
        tracing::error!("a document job panicked; the document owner keeps running");
    }
}

impl DocumentJobs {
    /// Run every job queued so far on `doc` (without blocking) and return how
    /// many ran. Jobs queued while draining run in the next call.
    pub fn drain(&self, doc: &mut Document) -> usize {
        let pending = self.rx.len();
        let mut n = 0;
        while n < pending {
            match self.rx.try_recv() {
                Ok(job) => {
                    run_job(job, doc);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }

    /// Number of jobs waiting.
    pub fn pending(&self) -> usize {
        self.rx.len()
    }

    /// Block until the next job arrives and run it. Returns `false` once all
    /// handles have been dropped (no more jobs can arrive).
    pub fn run_next_blocking(&self, doc: &mut Document) -> bool {
        match self.rx.recv() {
            Ok(job) => {
                run_job(job, doc);
                true
            }
            Err(_) => false,
        }
    }
}

impl DocumentHandle {
    /// Embedded mode: a handle plus the job receiver for the thread that owns
    /// the document.
    pub fn channel() -> (DocumentHandle, DocumentJobs) {
        Self::channel_inner(None)
    }

    /// Embedded mode with a waker that is called every time a job is queued
    /// (use it to request a GUI redraw so queued jobs are drained promptly).
    pub fn channel_with_waker(waker: Waker) -> (DocumentHandle, DocumentJobs) {
        Self::channel_inner(Some(waker))
    }

    fn channel_inner(waker: Option<Waker>) -> (DocumentHandle, DocumentJobs) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (DocumentHandle { tx, waker }, DocumentJobs { rx })
    }

    /// Queue a job without waiting for it. Fails when the owner is gone.
    pub fn submit(&self, job: DocumentJob) -> ApiResult<()> {
        self.tx
            .send(job)
            .map_err(|_| ApiError::document_unavailable())?;
        if let Some(w) = &self.waker {
            w();
        }
        Ok(())
    }

    /// Run `f` on the document owner's thread and await its result.
    pub async fn run<R, F>(&self, f: F) -> ApiResult<R>
    where
        F: FnOnce(&mut Document) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.submit(Box::new(move |doc| {
            let _ = reply_tx.send(f(doc));
        }))?;
        reply_rx.await.map_err(|_| {
            ApiError::internal("the document job did not complete (it panicked or the document owner shut down)")
        })
    }

    /// Blocking variant of [`DocumentHandle::run`] for synchronous callers.
    ///
    /// Must not be called from the thread that owns the document (embedded
    /// mode) or from inside an async task: it would deadlock / block the
    /// executor.
    pub fn run_blocking<R, F>(&self, f: F) -> ApiResult<R>
    where
        F: FnOnce(&mut Document) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.submit(Box::new(move |doc| {
            let _ = reply_tx.send(f(doc));
        }))?;
        reply_rx.recv().map_err(|_| {
            ApiError::internal("the document job did not complete (it panicked or the document owner shut down)")
        })
    }
}

/// Headless host: a dedicated thread owning the document.
pub struct DocumentHost;

/// Join handle of a headless host thread; returns the document when every
/// [`DocumentHandle`] has been dropped.
pub type DocumentHostThread = JoinHandle<Document>;

impl DocumentHost {
    /// Start an owner thread for `doc` and return a handle to it. The thread
    /// ends when the last handle is dropped.
    pub fn spawn(doc: Document) -> DocumentHandle {
        Self::spawn_with_thread(doc).0
    }

    /// Like [`DocumentHost::spawn`], also returning the thread's join handle,
    /// which yields the final document once all handles are dropped.
    pub fn spawn_with_thread(mut doc: Document) -> (DocumentHandle, DocumentHostThread) {
        let (handle, jobs) = DocumentHandle::channel();
        let thread = std::thread::Builder::new()
            .name("forge3d-document".into())
            .spawn(move || {
                while jobs.run_next_blocking(&mut doc) {}
                doc
            })
            .expect("spawn document owner thread");
        (handle, thread)
    }
}
