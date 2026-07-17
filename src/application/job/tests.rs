use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;

use super::{
    ClaimedJob, JobDisposition, JobHandler, JobHandlerError, JobQueue, JobQueueError, JobWorker,
    NewJob, RunOutcome,
};

#[derive(Default)]
struct FakeQueue {
    jobs: Mutex<VecDeque<ClaimedJob>>,
    completed: Mutex<Vec<Uuid>>,
    failed: Mutex<Vec<(Uuid, Duration)>>,
}

#[async_trait]
impl JobQueue for FakeQueue {
    async fn enqueue(&self, job: &NewJob) -> Result<(), JobQueueError> {
        self.jobs.lock().unwrap().push_back(ClaimedJob {
            id: job.id,
            job_type: job.job_type.clone(),
            payload: job.payload.clone(),
            attempts: 1,
            max_attempts: job.max_attempts,
        });
        Ok(())
    }

    async fn claim(
        &self,
        _worker_id: &str,
        _lease_timeout: Duration,
    ) -> Result<Option<ClaimedJob>, JobQueueError> {
        Ok(self.jobs.lock().unwrap().pop_front())
    }

    async fn complete(&self, job_id: Uuid, _worker_id: &str) -> Result<(), JobQueueError> {
        self.completed.lock().unwrap().push(job_id);
        Ok(())
    }

    async fn fail(
        &self,
        job_id: Uuid,
        _worker_id: &str,
        _error: &str,
        retry_delay: Duration,
    ) -> Result<JobDisposition, JobQueueError> {
        self.failed.lock().unwrap().push((job_id, retry_delay));
        Ok(JobDisposition::RetryScheduled)
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

fn claimed_job(job_type: &str, attempts: u32) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::new_v4(),
        job_type: job_type.to_owned(),
        payload: json!({}),
        attempts,
        max_attempts: 5,
    }
}

#[tokio::test]
async fn completes_a_job_with_a_registered_handler() {
    let queue = Arc::new(FakeQueue::default());
    let job = claimed_job("test.success", 1);
    queue.jobs.lock().unwrap().push_back(job.clone());
    let worker = JobWorker::new(
        queue.clone(),
        vec![Arc::new(SuccessfulHandler)],
        "worker-1".to_owned(),
        Duration::from_secs(30),
        Duration::from_secs(5),
        Duration::from_secs(60),
    );

    assert_eq!(worker.run_once().await.unwrap(), RunOutcome::Completed);
    assert_eq!(queue.completed.lock().unwrap().as_slice(), &[job.id]);
}

#[tokio::test]
async fn retries_an_unknown_job_with_exponential_backoff() {
    let queue = Arc::new(FakeQueue::default());
    let job = claimed_job("test.unknown", 3);
    queue.jobs.lock().unwrap().push_back(job.clone());
    let worker = JobWorker::new(
        queue.clone(),
        Vec::new(),
        "worker-1".to_owned(),
        Duration::from_secs(30),
        Duration::from_secs(5),
        Duration::from_secs(60),
    );

    assert_eq!(worker.run_once().await.unwrap(), RunOutcome::RetryScheduled);
    assert_eq!(
        queue.failed.lock().unwrap().as_slice(),
        &[(job.id, Duration::from_secs(20))]
    );
}
