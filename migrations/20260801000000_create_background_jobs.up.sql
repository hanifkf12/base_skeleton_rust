CREATE TABLE background_jobs (
    id UUID PRIMARY KEY,
    job_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    locked_by TEXT,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    CONSTRAINT background_jobs_type_valid
        CHECK (CHAR_LENGTH(BTRIM(job_type)) BETWEEN 1 AND 200),
    CONSTRAINT background_jobs_status_valid
        CHECK (status IN ('pending', 'running', 'completed', 'dead')),
    CONSTRAINT background_jobs_attempts_nonnegative CHECK (attempts >= 0),
    CONSTRAINT background_jobs_max_attempts_positive CHECK (max_attempts > 0),
    CONSTRAINT background_jobs_attempts_bounded CHECK (attempts <= max_attempts),
    CONSTRAINT background_jobs_lock_consistent CHECK (
        (status = 'running' AND locked_at IS NOT NULL AND locked_by IS NOT NULL)
        OR
        (status <> 'running' AND locked_at IS NULL AND locked_by IS NULL)
    ),
    CONSTRAINT background_jobs_completion_consistent CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR
        (status <> 'completed' AND completed_at IS NULL)
    )
);

CREATE INDEX background_jobs_pending_idx
    ON background_jobs (available_at, created_at)
    WHERE status = 'pending';

CREATE INDEX background_jobs_running_lease_idx
    ON background_jobs (locked_at)
    WHERE status = 'running';

CREATE INDEX background_jobs_type_status_idx
    ON background_jobs (job_type, status);
