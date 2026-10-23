DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'users_email_key'
          AND connamespace = to_regnamespace(current_schema())
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'users_email_unique'
          AND connamespace = to_regnamespace(current_schema())
    ) THEN
        ALTER TABLE users RENAME CONSTRAINT users_email_key TO users_email_unique;
    END IF;
END
$$;
