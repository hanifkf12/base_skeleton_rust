ALTER TABLE background_jobs
    ADD COLUMN trace_context JSONB NOT NULL DEFAULT '{}'::jsonb;
