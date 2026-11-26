//! Pure state machine for a queue of parallel transfers.
//!
//! No threads, timers or I/O live here: the GUI owns the workers and reports
//! byte counts through [`TransferQueue::set_progress`] and
//! [`TransferQueue::finish`]. Keeping the bookkeeping display-free makes the
//! transition rules easy to test on a machine without a display.

/// Which way a job moves bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// Where a job stands. Running and paused jobs keep the last byte counts so a
/// progress bar can be redrawn after a pause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running { done: u64, total: u64 },
    Paused { done: u64, total: u64 },
    Done,
    Failed(String),
    Cancelled,
}

/// One transfer, with a stable id assigned by the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferJob {
    pub id: u64,
    pub direction: TransferDirection,
    pub local: String,
    pub remote: String,
    pub state: TransferState,
}

/// An ordered list of jobs with a cap on how many may run at once.
#[derive(Clone, Debug)]
pub struct TransferQueue {
    jobs: Vec<TransferJob>,
    next_id: u64,
    max_concurrent: usize,
}

impl TransferQueue {
    /// Create an empty queue. A limit of zero would never schedule anything,
    /// so it is treated as one.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// Append a queued job and return its id.
    pub fn enqueue(&mut self, direction: TransferDirection, local: String, remote: String) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.jobs.push(TransferJob {
            id,
            direction,
            local,
            remote,
            state: TransferState::Queued,
        });
        id
    }

    fn find(&self, id: u64) -> Option<&TransferJob> {
        self.jobs.iter().find(|job| job.id == id)
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut TransferJob> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    fn missing(id: u64) -> String {
        format!("No transfer with id {id}")
    }

    /// Queued -> Running. The caller is responsible for asking
    /// [`Self::next_schedulable`] first when it wants to respect the limit.
    pub fn start(&mut self, id: u64) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        if !matches!(&job.state, TransferState::Queued) {
            return Err(format!("Transfer {id} cannot start from {:?}", job.state));
        }
        job.state = TransferState::Running { done: 0, total: 0 };
        Ok(())
    }

    /// Running -> Paused, keeping the progress reported so far.
    pub fn pause(&mut self, id: u64) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        match &job.state {
            TransferState::Running { done, total } => {
                let (done, total) = (*done, *total);
                job.state = TransferState::Paused { done, total };
                Ok(())
            }
            _ => Err(format!("Transfer {id} is not running")),
        }
    }

    /// Paused -> Running, keeping the progress reported so far.
    pub fn resume(&mut self, id: u64) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        match &job.state {
            TransferState::Paused { done, total } => {
                let (done, total) = (*done, *total);
                job.state = TransferState::Running { done, total };
                Ok(())
            }
            _ => Err(format!("Transfer {id} is not paused")),
        }
    }

    /// Queued, Running or Paused -> Cancelled. Cancelling twice is harmless;
    /// a job that already finished cannot be cancelled.
    pub fn cancel(&mut self, id: u64) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        if matches!(&job.state, TransferState::Cancelled) {
            return Ok(());
        }
        if matches!(&job.state, TransferState::Done | TransferState::Failed(_)) {
            return Err(format!("Transfer {id} has already finished"));
        }
        job.state = TransferState::Cancelled;
        Ok(())
    }

    /// Update the byte counts of a running or paused job.
    pub fn set_progress(&mut self, id: u64, done: u64, total: u64) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        match &job.state {
            TransferState::Running { .. } => {
                job.state = TransferState::Running { done, total };
                Ok(())
            }
            TransferState::Paused { .. } => {
                job.state = TransferState::Paused { done, total };
                Ok(())
            }
            _ => Err(format!("Transfer {id} is not running or paused")),
        }
    }

    /// Running -> Done or Failed. A paused or queued job cannot finish.
    pub fn finish(&mut self, id: u64, result: Result<(), String>) -> Result<(), String> {
        let job = self.find_mut(id).ok_or_else(|| Self::missing(id))?;
        if !matches!(&job.state, TransferState::Running { .. }) {
            return Err(format!("Transfer {id} is not running"));
        }
        job.state = match result {
            Ok(()) => TransferState::Done,
            Err(message) => TransferState::Failed(message),
        };
        Ok(())
    }

    /// Jobs that have not reached a terminal state, in insertion order.
    pub fn pending(&self) -> Vec<&TransferJob> {
        self.jobs
            .iter()
            .filter(|job| {
                !matches!(
                    job.state,
                    TransferState::Done | TransferState::Failed(_) | TransferState::Cancelled
                )
            })
            .collect()
    }

    /// All jobs, finished ones included.
    pub fn jobs(&self) -> &[TransferJob] {
        &self.jobs
    }

    /// Only the jobs still waiting to start.
    pub fn queued(&self) -> Vec<&TransferJob> {
        self.jobs
            .iter()
            .filter(|job| matches!(job.state, TransferState::Queued))
            .collect()
    }

    /// Number of jobs transferring bytes right now.
    pub fn active_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|job| matches!(job.state, TransferState::Running { .. }))
            .count()
    }

    /// Queued jobs that fit in the free slots, in insertion order.
    pub fn next_schedulable(&self) -> Vec<&TransferJob> {
        let free = self.max_concurrent.saturating_sub(self.active_count());
        self.jobs
            .iter()
            .filter(|job| matches!(job.state, TransferState::Queued))
            .take(free)
            .collect()
    }

    /// One job by id, if it exists.
    pub fn get(&self, id: u64) -> Option<&TransferJob> {
        self.find(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduling_respects_the_two_slot_limit() {
        let mut queue = TransferQueue::new(2);
        let first = queue.enqueue(TransferDirection::Upload, "a".into(), "/a".into());
        let second = queue.enqueue(TransferDirection::Download, "b".into(), "/b".into());
        let third = queue.enqueue(TransferDirection::Upload, "c".into(), "/c".into());
        assert_eq!((first, second, third), (1, 2, 3));

        let ids: Vec<u64> = queue.next_schedulable().iter().map(|job| job.id).collect();
        assert_eq!(ids, vec![1, 2]);
        queue.start(first).unwrap();
        queue.start(second).unwrap();
        assert_eq!(queue.active_count(), 2);
        assert!(queue.next_schedulable().is_empty());

        queue.finish(first, Ok(())).unwrap();
        assert_eq!(queue.active_count(), 1);
        let ids: Vec<u64> = queue.next_schedulable().iter().map(|job| job.id).collect();
        assert_eq!(ids, vec![3]);
        queue.start(third).unwrap();
        assert_eq!(queue.active_count(), 2);
        assert!(queue.next_schedulable().is_empty());
    }

    #[test]
    fn pause_and_resume_keep_progress() {
        let mut queue = TransferQueue::new(1);
        let id = queue.enqueue(TransferDirection::Upload, "a".into(), "/a".into());
        queue.start(id).unwrap();
        queue.set_progress(id, 25, 100).unwrap();
        assert_eq!(
            queue.get(id).unwrap().state,
            TransferState::Running {
                done: 25,
                total: 100
            }
        );

        queue.pause(id).unwrap();
        assert_eq!(queue.active_count(), 0);
        assert_eq!(
            queue.get(id).unwrap().state,
            TransferState::Paused {
                done: 25,
                total: 100
            }
        );
        assert!(queue.start(id).is_err());
        assert!(queue.pause(id).is_err());

        queue.set_progress(id, 50, 100).unwrap();
        queue.resume(id).unwrap();
        assert_eq!(
            queue.get(id).unwrap().state,
            TransferState::Running {
                done: 50,
                total: 100
            }
        );
        assert!(queue.resume(id).is_err());
        queue.finish(id, Ok(())).unwrap();
        assert_eq!(queue.get(id).unwrap().state, TransferState::Done);
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn cancel_and_failure_are_terminal() {
        let mut queue = TransferQueue::new(2);
        let cancelled = queue.enqueue(TransferDirection::Download, "a".into(), "/a".into());
        let failed = queue.enqueue(TransferDirection::Download, "b".into(), "/b".into());
        queue.start(cancelled).unwrap();
        queue.cancel(cancelled).unwrap();
        assert_eq!(
            queue.get(cancelled).unwrap().state,
            TransferState::Cancelled
        );
        // Dropping an already cancelled job is fine.
        queue.cancel(cancelled).unwrap();
        assert!(queue.resume(cancelled).is_err());
        assert!(queue.finish(cancelled, Ok(())).is_err());

        queue.start(failed).unwrap();
        queue
            .finish(failed, Err("permission denied".into()))
            .unwrap();
        assert_eq!(
            queue.get(failed).unwrap().state,
            TransferState::Failed("permission denied".into())
        );
        assert!(queue.cancel(failed).is_err());
        assert!(queue.start(failed).is_err());
        assert!(queue.finish(failed, Ok(())).is_err());
        assert!(queue.pending().is_empty());
        assert_eq!(queue.jobs().len(), 2);
    }

    #[test]
    fn unknown_ids_are_errors() {
        let mut queue = TransferQueue::new(1);
        assert_eq!(queue.start(7), Err("No transfer with id 7".into()));
        assert!(queue.pause(7).is_err());
        assert!(queue.resume(7).is_err());
        assert!(queue.cancel(7).is_err());
        assert!(queue.set_progress(7, 1, 2).is_err());
        assert!(queue.finish(7, Ok(())).is_err());
        assert!(queue.get(7).is_none());
        assert!(queue.jobs().is_empty());
    }

    #[test]
    fn a_zero_limit_still_allows_one_transfer() {
        let mut queue = TransferQueue::new(0);
        let id = queue.enqueue(TransferDirection::Upload, "a".into(), "/a".into());
        assert_eq!(queue.next_schedulable().len(), 1);
        queue.start(id).unwrap();
        assert_eq!(queue.active_count(), 1);
        assert!(queue.next_schedulable().is_empty());
    }
}
