-- #604 Fix 2: bounded retries with exponential backoff + quarantine for
-- failed ingestion files. Additive only — never edit existing migrations.
-- status has no CHECK constraint, so the new terminal 'quarantined' value
-- needs no schema change.
ALTER TABLE ingestion_files ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ingestion_files ADD COLUMN next_attempt_at INTEGER;
