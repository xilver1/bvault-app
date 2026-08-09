-- Submission batches: the durable, user-facing record of an "analyze these
-- playlists" request. Deliberately separate from `jobs` — a job is ephemeral
-- queue work (prunable once succeeded), a batch is a stable id the client keeps
-- polling plus a label for the completion notification.
-- Requires PostgreSQL 13+ for the built-in gen_random_uuid().
--
-- Progress is DERIVED, never stored: `done` from artifact-store presence over
-- `hashes`, `failed` from the jobs dead-letter. So this table holds only the
-- submitted hash-set snapshot — no mutable counter that could drift or lie once
-- succeeded jobs are pruned.
create table batches (
    id         uuid primary key default gen_random_uuid(),
    -- Snapshot of the submission's full member hash set: ALL selected tracks,
    -- including any already analyzed at submit time, so the denominator is
    -- honest (18-of-20-already-done reads as 18/20, not 2/2).
    hashes     text[]      not null,
    name       text,                    -- optional, for the "analysis complete" notification
    created_at timestamptz not null default now()
);