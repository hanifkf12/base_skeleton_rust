use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use tokio::time::Instant;
use tracing::Span;
use uuid::Uuid;

use super::{
    ClaimedJob, JobDisposition, JobHandler, JobHandlerError, JobQueue, JobQueueError, JobTracer,
    JobWorker, JobWorkerConfig, NewJob, RunOutcome,
};

#[derive(Clone, Copy, Default)]
enum Renewal {
    #[default]
    Healthy,
    Lost,
    Unavailable,
    Stalled,
}

struct Entry {
    job: ClaimedJob,
    status: &'static str,
    owner: String,
    expires: Instant,
    available: Instant,
    lease: Duration,
}

#[derive(Default)]
struct FakeQueue {
    entries: Mutex<VecDeque<Entry>>,
    renewal: Renewal,
}

impl FakeQueue {
    fn with_job(job: ClaimedJob, renewal: Renewal) -> Self {
        Self {
            entries: Mutex::new(VecDeque::from([Entry {
                job,
                status: "pending",
                owner: String::new(),
                expires: Instant::now(),
                available: Instant::now(),
                lease: Duration::ZERO,
            }])),
            renewal,
        }
    }

    fn owned(entry: &Entry, id: Uuid, worker: &str, attempt: u32) -> bool {
        entry.job.id == id
            && entry.status == "running"
            && entry.owner == worker
            && entry.job.attempts == attempt
            && entry.expires > Instant::now()
    }
}

#[async_trait]
impl JobQueue for FakeQueue {
    async fn enqueue(&self, job: &NewJob) -> Result<(), JobQueueError> {
        self.entries.lock().push_back(Entry {
            job: ClaimedJob {
                id: job.id,
                job_type: job.job_type.clone(),
                payload: job.payload.clone(),
                trace_context: job.trace_context.clone(),
                attempts: 0,
                max_attempts: job.max_attempts,
            },
            status: "pending",
            owner: String::new(),
            expires: Instant::now(),
            available: Instant::now(),
            lease: Duration::ZERO,
        });
        Ok(())
    }

    async fn claim(
        &self,
        worker_id: &str,
        lease_timeout: Duration,
    ) -> Result<Option<ClaimedJob>, JobQueueError> {
        let mut entries = self.entries.lock();
        for entry in entries.iter_mut() {
            if entry.status == "running" && entry.expires <= Instant::now() {
                entry.status = if entry.job.attempts >= entry.job.max_attempts {
                    "dead"
                } else {
                    "pending"
                };
            }
            if entry.status == "pending" && entry.available <= Instant::now() {
                entry.status = "running";
                entry.job.attempts += 1;
                entry.owner = worker_id.to_owned();
                entry.lease = lease_timeout;
                entry.expires = Instant::now() + lease_timeout;
                return Ok(Some(entry.job.clone()));
            }
        }
        Ok(None)
    }

    async fn complete(&self, id: Uuid, worker: &str, attempt: u32) -> Result<(), JobQueueError> {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|entry| Self::owned(entry, id, worker, attempt))
            .ok_or(JobQueueError::LeaseLost)?;
        entry.status = "completed";
        Ok(())
    }

    async fn renew(&self, id: Uuid, worker: &str, attempt: u32) -> Result<(), JobQueueError> {
        match self.renewal {
            Renewal::Stalled => std::future::pending().await,
            Renewal::Unavailable => Err(JobQueueError::Unavailable),
            Renewal::Lost => {
                let mut entries = self.entries.lock();
                let entry = entries.iter_mut().find(|entry| entry.job.id == id).unwrap();
                // Simulate reclaim by another incarnation with the same worker ID.
                entry.job.attempts += 1;
                Err(JobQueueError::LeaseLost)
            }
            Renewal::Healthy => {
                let mut entries = self.entries.lock();
                let entry = entries
                    .iter_mut()
                    .find(|entry| Self::owned(entry, id, worker, attempt))
                    .ok_or(JobQueueError::LeaseLost)?;
                entry.expires = Instant::now() + entry.lease;
                Ok(())
            }
        }
    }

    async fn fail(
        &self,
        id: Uuid,
        worker: &str,
        attempt: u32,
        _error: &str,
        retry_delay: Duration,
    ) -> Result<JobDisposition, JobQueueError> {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|entry| Self::owned(entry, id, worker, attempt))
            .ok_or(JobQueueError::LeaseLost)?;
        entry.available = Instant::now() + retry_delay;
        if entry.job.attempts >= entry.job.max_attempts {
            entry.status = "dead";
            Ok(JobDisposition::DeadLettered)
        } else {
            entry.status = "pending";
            Ok(JobDisposition::RetryScheduled)
        }
    }

    async fn purge_terminal(
        &self,
        _completed: Duration,
        _dead: Duration,
    ) -> Result<u64, JobQueueError> {
        Ok(0)
    }
}

struct SuccessfulHandler;

#[async_trait]
impl JobHandler for SuccessfulHandler {
    fn job_type(&self) -> &'static str {
        "test.success"
    }
    async fn handle(&self, _job: &ClaimedJob) -> Result<(), JobHandlerError> {
        Ok(())
    }
}

struct DelayedHandler {
    duration: Duration,
    effects: Arc<AtomicUsize>,
}

#[async_trait]
impl JobHandler for DelayedHandler {
    fn job_type(&self) -> &'static str {
        "test.success"
    }
    async fn handle(&self, _job: &ClaimedJob) -> Result<(), JobHandlerError> {
        tokio::time::sleep(self.duration).await;
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn claimed_job(job_type: &str, attempts: u32) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::new_v4(),
        job_type: job_type.to_owned(),
        payload: json!({}),
        trace_context: json!({}),
        attempts,
        max_attempts: 5,
    }
}

struct NoOpJobTracer;
impl JobTracer for NoOpJobTracer {
    fn span(&self, _job: &ClaimedJob) -> Span {
        Span::none()
    }
}

fn worker(queue: Arc<dyn JobQueue>, handlers: Vec<Arc<dyn JobHandler>>) -> JobWorker {
    JobWorker::new(
        queue,
        handlers,
        Arc::new(NoOpJobTracer),
        JobWorkerConfig {
            worker_id: "worker-1".to_owned(),
            lease_timeout: Duration::from_secs(9),
            retry_base: Duration::from_secs(5),
            retry_max: Duration::from_secs(60),
            completed_retention: Duration::from_secs(86_400),
            dead_retention: Duration::from_secs(2_592_000),
        },
    )
}

#[tokio::test(start_paused = true)]
async fn completes_a_job_with_a_registered_handler() {
    let queue = Arc::new(FakeQueue::with_job(
        claimed_job("test.success", 0),
        Renewal::Healthy,
    ));
    let worker = worker(queue.clone(), vec![Arc::new(SuccessfulHandler)]);
    assert_eq!(worker.run_once().await.unwrap(), RunOutcome::Completed);
    assert_eq!(queue.entries.lock()[0].status, "completed");
    assert!(
        queue
            .claim("other-worker", Duration::from_secs(9))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn retries_an_unknown_job_with_exponential_backoff() {
    let queue = Arc::new(FakeQueue::with_job(
        claimed_job("test.unknown", 2),
        Renewal::Healthy,
    ));
    let worker = worker(queue.clone(), Vec::new());
    assert_eq!(worker.run_once().await.unwrap(), RunOutcome::RetryScheduled);
    assert_eq!(queue.entries.lock()[0].status, "pending");
    tokio::time::advance(Duration::from_secs(19)).await;
    assert!(
        queue
            .claim("other", Duration::from_secs(9))
            .await
            .unwrap()
            .is_none()
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        queue
            .claim("other", Duration::from_secs(9))
            .await
            .unwrap()
            .unwrap()
            .attempts,
        4
    );
}

#[tokio::test(start_paused = true)]
async fn heartbeat_keeps_a_long_handler_owned() {
    let queue = Arc::new(FakeQueue::with_job(
        claimed_job("test.success", 0),
        Renewal::Healthy,
    ));
    let effects = Arc::new(AtomicUsize::new(0));
    let worker = worker(
        queue.clone(),
        vec![Arc::new(DelayedHandler {
            duration: Duration::from_secs(40),
            effects: effects.clone(),
        })],
    );
    let execution = tokio::spawn(async move { worker.run_once().await });
    tokio::task::yield_now().await;
    for _ in 0..13 {
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        assert!(
            queue
                .claim("competitor", Duration::from_secs(9))
                .await
                .unwrap()
                .is_none()
        );
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(execution.await.unwrap().unwrap(), RunOutcome::Completed);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let entries = queue.entries.lock();
    assert_eq!(entries[0].status, "completed");
    assert_eq!(entries[0].job.attempts, 1);
}

#[tokio::test(start_paused = true)]
async fn renewal_failures_cancel_the_handler_without_terminal_writes() {
    for (mode, expected, attempts) in [
        (Renewal::Lost, JobQueueError::LeaseLost, 2),
        (Renewal::Unavailable, JobQueueError::Unavailable, 1),
        (Renewal::Stalled, JobQueueError::LeaseLost, 1),
    ] {
        let queue = Arc::new(FakeQueue::with_job(claimed_job("test.success", 0), mode));
        let effects = Arc::new(AtomicUsize::new(0));
        let worker = worker(
            queue.clone(),
            vec![Arc::new(DelayedHandler {
                duration: Duration::from_secs(60),
                effects: effects.clone(),
            })],
        );
        let started = Instant::now();
        assert_eq!(worker.run_once().await.unwrap_err(), expected);
        assert!(started.elapsed() <= Duration::from_secs(9));
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        let entries = queue.entries.lock();
        assert_eq!(entries[0].status, "running");
        assert_eq!(entries[0].job.attempts, attempts);
    }
}

#[tokio::test(start_paused = true)]
async fn handler_finishing_during_a_stalled_renewal_does_not_complete_the_job() {
    let queue = Arc::new(FakeQueue::with_job(
        claimed_job("test.success", 0),
        Renewal::Stalled,
    ));
    let effects = Arc::new(AtomicUsize::new(0));
    let worker = worker(
        queue.clone(),
        vec![Arc::new(DelayedHandler {
            duration: Duration::from_secs(4),
            effects: effects.clone(),
        })],
    );
    let started = Instant::now();
    assert_eq!(
        worker.run_once().await.unwrap_err(),
        JobQueueError::LeaseLost
    );
    assert_eq!(started.elapsed(), Duration::from_secs(9));
    // An effect committed before lease loss is not reversible; only the stale
    // terminal write is prevented after renewal fails to establish ownership.
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(queue.entries.lock()[0].status, "running");
}
