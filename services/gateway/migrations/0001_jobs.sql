-- Job queue: Postgres-backed, claimed with FOR UPDATE SKIP LOCKED under a
-- visibility-timeout lease. No external broker — the DB the gateway already
-- runs is the queue, which suits a RAM-limited cluster.

-- One value for now; `alter type job_kind add value 'export'` when export lands.
create type job_kind as enum ('analysis');

create type job_status as enum ('pending', 'running', 'succeeded', 'failed', 'dead');

create table jobs (
    id           bigint generated always as identity primary key,
    kind         job_kind    not null,
    -- Idempotency key: the content hash (hex) for analysis.
    dedup_key    text        not null,
    -- Kind-specific contract, e.g. { hash, raw_location } for analysis.
    payload      jsonb       not null,
    status       job_status  not null default 'pending',
    attempts     integer     not null default 0,
    max_attempts integer     not null default 5,
    -- Lease: set on claim, cleared on terminal state.
    locked_at    timestamptz,
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now(),
    finished_at  timestamptz
);

-- At most one *active* job per (kind, dedup_key): idempotent enqueue. A hash
-- already pending or running cannot be enqueued twice.
create unique index jobs_active_dedup
    on jobs (kind, dedup_key)
    where status in ('pending', 'running');

-- Claim scan: runnable = pending, or running with an expired lease. Ordered by
-- age so the queue is FIFO-ish.
create index jobs_claim
    on jobs (kind, status, locked_until, created_at);