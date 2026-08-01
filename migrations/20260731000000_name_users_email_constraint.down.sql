DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'users_email_unique'
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'users_email_key'
    ) THEN
        ALTER TABLE users RENAME CONSTRAINT users_email_unique TO users_email_key;
    END IF;
END
$$;
