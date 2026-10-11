//! Background export queue (one job at a time, FIFO).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Sender, unbounded};
use cutroom_media::MediaBackend;
use parking_lot::Mutex;

use crate::pipeline::{ExportError, ExportJob, Progress, run_export};

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    Running(Progress),
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

struct Entry {
    state: JobState,
    cancel: Arc<AtomicBool>,
}

pub struct ExportQueue {
    tx: Sender<(JobId, ExportJob)>,
    jobs: Arc<Mutex<BTreeMap<JobId, Entry>>>,
    next: Mutex<JobId>,
}

impl ExportQueue {
    pub fn new(
        backend: Arc<dyn MediaBackend>,
        ffmpeg: PathBuf,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = unbounded::<(JobId, ExportJob)>();
        let jobs: Arc<Mutex<BTreeMap<JobId, Entry>>> = Arc::default();
        let jobs2 = jobs.clone();
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                for (id, job) in rx {
                    let Some(cancel) = jobs2.lock().get(&id).map(|e| e.cancel.clone()) else {
                        continue;
                    };
                    if cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let set = |s: JobState| {
                        if let Some(e) = jobs2.lock().get_mut(&id) {
                            e.state = s;
                        }
                    };
                    let res = run_export(&job, backend.clone(), &ffmpeg, &cancel, |p| {
                        set(JobState::Running(p));
                        wake();
                    });
                    set(match res {
                        Ok(()) => JobState::Done(job.output.clone()),
                        Err(ExportError::Cancelled) => JobState::Cancelled,
                        Err(e) => JobState::Failed(e.to_string()),
                    });
                    wake();
                }
            })
            .expect("spawn export thread");
        ExportQueue {
            tx,
            jobs,
            next: Mutex::new(0),
        }
    }

    pub fn submit(&self, job: ExportJob) -> JobId {
        let id = {
            let mut n = self.next.lock();
            *n += 1;
            *n
        };
        self.jobs.lock().insert(
            id,
            Entry {
                state: JobState::Queued,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        let _ = self.tx.send((id, job));
        id
    }

    pub fn cancel(&self, id: JobId) {
        if let Some(e) = self.jobs.lock().get_mut(&id) {
            e.cancel.store(true, Ordering::Relaxed);
            if e.state == JobState::Queued {
                e.state = JobState::Cancelled;
            }
        }
    }

    pub fn state(&self, id: JobId) -> Option<JobState> {
        self.jobs.lock().get(&id).map(|e| e.state.clone())
    }

    pub fn all(&self) -> Vec<(JobId, JobState)> {
        self.jobs
            .lock()
            .iter()
            .map(|(k, v)| (*k, v.state.clone()))
            .collect()
    }
}
