-- Authentication: self-hosted accounts + opaque server-side sessions, living in
-- the Postgres the gateway already runs — no external identity provider. This is
-- the first table that introduces a per-user owner; the tenancy columns on
-- tracks/playlists/batches land in the next migration and reference users(id).
-- Requires PostgreSQL 13+ for gen_random_uuid().
--
-- Passwords are never stored — only an Argon2id PHC string (algorithm, params,
-- salt, digest). Session tokens are never stored either: the client holds the
-- raw 256-bit token, the DB holds only its SHA-256 fingerprint, so a read-only
-- leak of this table yields nothing replayable. Authority is a row's existence
-- and expiry, so logout/revocation is a single delete.

create table users (
    id            uuid        primary key default gen_random_uuid(),
    username      text        not null,
    password_hash text        not null,   -- Argon2id PHC string
    created_at    timestamptz not null default now()
);

-- Case-insensitive uniqueness and lookup without the citext extension: both go
-- through lower(username).
create unique index users_username_lower on users (lower(username));

create table sessions (
    -- SHA-256 hex of the bearer token the client was handed — never the token.
    token_hash text        primary key,
    user_id    uuid        not null references users (id) on delete cascade,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null
);

-- Lazy expiry sweeps and (rare) per-user session enumeration / bulk revoke.
create index sessions_expires_at on sessions (expires_at);
create index sessions_user_id on sessions (user_id);
