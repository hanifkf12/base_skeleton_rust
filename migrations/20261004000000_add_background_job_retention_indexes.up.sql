CREATE INDEX background_jobs_completed_retention_idx
    ON background_jobs (completed_at)
    WHERE status = 'completed';

CREATE INDEX background_jobs_dead_retention_idx
    ON background_jobs (updated_at)
    WHERE status = 'dead';
