#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};

use free_whisper_domain::{AppState, JobId, JobStateMachine, StateTransitionError};
use thiserror::Error;

/// The capture half of a transcription lifecycle. Processing is intentionally
/// tracked separately by the desktop host so a user can record the next job
/// while one finalized recording is being transcribed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CapturePhase {
    #[default]
    Idle,
    Preparing {
        job_id: JobId,
    },
    Recording {
        job_id: JobId,
    },
    Finalizing {
        job_id: JobId,
    },
}

impl CapturePhase {
    #[must_use]
    pub const fn job_id(self) -> Option<JobId> {
        match self {
            Self::Idle => None,
            Self::Preparing { job_id }
            | Self::Recording { job_id }
            | Self::Finalizing { job_id } => Some(job_id),
        }
    }
}

/// Result of committing a prepared capture after asynchronous provider and
/// device setup. A cancellation may have won while setup was in progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparationCommit {
    Recording,
    Cancelled,
}

/// Describes the capture resource that cancellation acquired. The caller owns
/// its audio/provider cleanup, while this coordinator already made the slot
/// available to the next job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelledCapture {
    Preparing { job_id: JobId },
    Recording { job_id: JobId },
    Finalizing { job_id: JobId },
}

impl CancelledCapture {
    #[must_use]
    pub const fn job_id(self) -> JobId {
        match self {
            Self::Preparing { job_id }
            | Self::Recording { job_id }
            | Self::Finalizing { job_id } => job_id,
        }
    }
}

/// Synchronous authority for the microphone slot. It contains no I/O so the
/// desktop host never holds a lifecycle mutex across device, process, or
/// network operations.
#[derive(Debug, Default)]
pub struct RecordingCoordinator {
    capture: CapturePhase,
}

impl RecordingCoordinator {
    #[must_use]
    pub const fn capture_phase(&self) -> CapturePhase {
        self.capture
    }

    pub fn reserve_start(&mut self, job_id: JobId) -> Result<(), RecordingCoordinatorError> {
        match self.capture {
            CapturePhase::Idle => {
                self.capture = CapturePhase::Preparing { job_id };
                Ok(())
            }
            phase => Err(RecordingCoordinatorError::CaptureBusy {
                active_job_id: phase.job_id().expect("non-idle capture phase has a job"),
                phase,
            }),
        }
    }

    pub fn commit_preparation(
        &mut self,
        job_id: JobId,
    ) -> Result<PreparationCommit, RecordingCoordinatorError> {
        match self.capture {
            CapturePhase::Preparing {
                job_id: active_job_id,
            } if active_job_id == job_id => {
                self.capture = CapturePhase::Recording { job_id };
                Ok(PreparationCommit::Recording)
            }
            CapturePhase::Idle => Ok(PreparationCommit::Cancelled),
            phase => Err(RecordingCoordinatorError::JobOwnership {
                requested_job_id: job_id,
                active_job_id: phase.job_id(),
            }),
        }
    }

    pub fn begin_finalization(&mut self) -> Result<JobId, RecordingCoordinatorError> {
        match self.capture {
            CapturePhase::Recording { job_id } => {
                self.capture = CapturePhase::Finalizing { job_id };
                Ok(job_id)
            }
            CapturePhase::Finalizing { job_id } => {
                Err(RecordingCoordinatorError::AlreadyFinalizing { job_id })
            }
            CapturePhase::Idle | CapturePhase::Preparing { .. } => {
                Err(RecordingCoordinatorError::NoActiveRecording {
                    phase: self.capture,
                })
            }
        }
    }

    pub fn release_finalization(&mut self, job_id: JobId) -> Result<(), RecordingCoordinatorError> {
        match self.capture {
            CapturePhase::Finalizing {
                job_id: active_job_id,
            } if active_job_id == job_id => {
                self.capture = CapturePhase::Idle;
                Ok(())
            }
            phase => Err(RecordingCoordinatorError::JobOwnership {
                requested_job_id: job_id,
                active_job_id: phase.job_id(),
            }),
        }
    }

    pub fn abandon_preparation(&mut self, job_id: JobId) -> Result<(), RecordingCoordinatorError> {
        match self.capture {
            CapturePhase::Preparing {
                job_id: active_job_id,
            } if active_job_id == job_id => {
                self.capture = CapturePhase::Idle;
                Ok(())
            }
            CapturePhase::Idle => Ok(()),
            phase => Err(RecordingCoordinatorError::JobOwnership {
                requested_job_id: job_id,
                active_job_id: phase.job_id(),
            }),
        }
    }

    pub fn cancel_capture(&mut self) -> Option<CancelledCapture> {
        let cancelled = match self.capture {
            CapturePhase::Idle => return None,
            CapturePhase::Preparing { job_id } => CancelledCapture::Preparing { job_id },
            CapturePhase::Recording { job_id } => CancelledCapture::Recording { job_id },
            // Finalization already owns an audio handle outside this mutex.
            // Leave it marked as finalizing so its owner can release itself.
            CapturePhase::Finalizing { job_id } => {
                return Some(CancelledCapture::Finalizing { job_id });
            }
        };
        self.capture = CapturePhase::Idle;
        Some(cancelled)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecordingCoordinatorError {
    #[error("capture is already {phase:?} for job {active_job_id}")]
    CaptureBusy {
        active_job_id: JobId,
        phase: CapturePhase,
    },
    #[error("job {job_id} is already finalizing")]
    AlreadyFinalizing { job_id: JobId },
    #[error("there is no stoppable recording while capture is {phase:?}")]
    NoActiveRecording { phase: CapturePhase },
    #[error("job {requested_job_id} attempted to change capture state owned by {active_job_id:?}")]
    JobOwnership {
        requested_job_id: JobId,
        active_job_id: Option<JobId>,
    },
}

/// A bounded FIFO queue. Capacity counts waiting jobs; the active job is owned
/// by the orchestrator and is never silently evicted.
#[derive(Debug)]
pub struct BoundedJobQueue<T> {
    capacity: usize,
    entries: VecDeque<T>,
}

impl<T> BoundedJobQueue<T> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    pub fn enqueue(&mut self, entry: T) -> Result<(), QueueError> {
        if self.entries.len() >= self.capacity {
            return Err(QueueError::Full {
                capacity: self.capacity,
            });
        }
        self.entries.push_back(entry);
        Ok(())
    }

    pub fn dequeue(&mut self) -> Option<T> {
        self.entries.pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QueueError {
    #[error("transcription queue is full (capacity {capacity})")]
    Full { capacity: usize },
}

/// Owns state machines keyed by the job that created them. This prevents a UI
/// event or late worker result from changing another job's state.
#[derive(Default, Debug)]
pub struct JobStateRegistry {
    jobs: HashMap<JobId, JobStateMachine>,
}

impl JobStateRegistry {
    pub fn register(&mut self, job_id: JobId) -> Result<(), JobRegistryError> {
        if self.jobs.contains_key(&job_id) {
            return Err(JobRegistryError::AlreadyRegistered(job_id));
        }
        self.jobs.insert(job_id, JobStateMachine::new(job_id));
        Ok(())
    }

    pub fn transition(
        &mut self,
        job_id: JobId,
        requested_by: JobId,
        next: AppState,
    ) -> Result<(), JobRegistryError> {
        let machine = self
            .jobs
            .get_mut(&job_id)
            .ok_or(JobRegistryError::UnknownJob(job_id))?;
        machine
            .transition_for(requested_by, next)
            .map_err(JobRegistryError::State)
    }

    #[must_use]
    pub fn state(&self, job_id: JobId) -> Option<AppState> {
        self.jobs.get(&job_id).map(JobStateMachine::state)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum JobRegistryError {
    #[error("job {0} is already registered")]
    AlreadyRegistered(JobId),
    #[error("job {0} is not registered")]
    UnknownJob(JobId),
    #[error(transparent)]
    State(#[from] StateTransitionError),
}

#[cfg(test)]
mod tests {
    use free_whisper_domain::AppState;

    use super::*;

    #[test]
    fn bounded_queue_never_discards_a_waiting_job() {
        let mut queue = BoundedJobQueue::new(1);
        queue.enqueue("first").expect("first entry fits");

        assert_eq!(
            queue.enqueue("second"),
            Err(QueueError::Full { capacity: 1 })
        );
        assert_eq!(queue.dequeue(), Some("first"));
        assert!(queue.is_empty());
    }

    #[test]
    fn registry_rejects_cross_job_transition() {
        let owner = JobId::new();
        let mut registry = JobStateRegistry::default();
        registry.register(owner).expect("registration succeeds");

        let result = registry.transition(owner, JobId::new(), AppState::PreparingRecording);

        assert!(matches!(result, Err(JobRegistryError::State(_))));
        assert_eq!(registry.state(owner), Some(AppState::Idle));
    }

    #[test]
    fn capture_start_is_reserved_before_asynchronous_setup() {
        let first = JobId::new();
        let mut coordinator = RecordingCoordinator::default();

        coordinator.reserve_start(first).expect("first reservation");
        let error = coordinator
            .reserve_start(JobId::new())
            .expect_err("second reservation is rejected");

        assert!(matches!(
            error,
            RecordingCoordinatorError::CaptureBusy { .. }
        ));
        assert_eq!(
            coordinator.capture_phase(),
            CapturePhase::Preparing { job_id: first }
        );
    }

    #[test]
    fn cancellation_during_preparation_prevents_a_late_capture_commit() {
        let job_id = JobId::new();
        let mut coordinator = RecordingCoordinator::default();
        coordinator.reserve_start(job_id).expect("reservation");

        assert_eq!(
            coordinator.cancel_capture(),
            Some(CancelledCapture::Preparing { job_id })
        );
        assert_eq!(
            coordinator.commit_preparation(job_id),
            Ok(PreparationCommit::Cancelled)
        );
        assert_eq!(coordinator.capture_phase(), CapturePhase::Idle);
    }

    #[test]
    fn duplicate_stop_is_explicit_and_cannot_take_another_job() {
        let job_id = JobId::new();
        let mut coordinator = RecordingCoordinator::default();
        coordinator.reserve_start(job_id).expect("reservation");
        assert_eq!(
            coordinator.commit_preparation(job_id),
            Ok(PreparationCommit::Recording)
        );
        assert_eq!(coordinator.begin_finalization(), Ok(job_id));
        assert_eq!(
            coordinator.begin_finalization(),
            Err(RecordingCoordinatorError::AlreadyFinalizing { job_id })
        );
        coordinator
            .release_finalization(job_id)
            .expect("owner releases finalization");
    }

    #[test]
    fn only_the_finalizing_job_can_release_the_capture_slot() {
        let owner = JobId::new();
        let mut coordinator = RecordingCoordinator::default();
        coordinator.reserve_start(owner).expect("reservation");
        coordinator.commit_preparation(owner).expect("commit");
        coordinator.begin_finalization().expect("stop");

        assert!(matches!(
            coordinator.release_finalization(JobId::new()),
            Err(RecordingCoordinatorError::JobOwnership { .. })
        ));
        assert_eq!(
            coordinator.capture_phase(),
            CapturePhase::Finalizing { job_id: owner }
        );
    }
}
