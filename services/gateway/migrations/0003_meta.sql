-- The relational metadata model: the raw-file lookup and playlists.
-- Requires PostgreSQL 13+ for the built-in gen_random_uuid().
--
-- Note what is absent: there is no "analysis lookup" / "analyzed" column. Whether
-- a track is analyzed is derived from the content-addressable artifact store's
-- presence, never stored here — the single source of truth for done-ness.

-- Raw-file lookup: content hash -> where the raw audio lives, plus light,
-- ingestion-known display metadata (tags still win at analysis time). Populated
-- by ingestion; read by the gateway.
create table tracks (
    hash         text primary key,            -- content hash (hex) = identity
    raw_location text        not null,         -- store-relative path in the music store
    title        text,
    artist       text,
    added_at     timestamptz not null default now()
);

-- Playlists: a name (+ optional description) and, separately, an unordered set
-- of member hashes. No ordering is stored — rekordbox re-sorts on playback.
create table playlists (
    id          uuid primary key default gen_random_uuid(),
    name        text        not null,
    description text,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

-- Membership as a set: a hash may appear in many playlists; a playlist has many
-- hashes. No position column, by design.
create table playlist_tracks (
    playlist_id uuid not null references playlists (id) on delete cascade,
    hash        text not null references tracks (hash) on delete cascade,
    primary key (playlist_id, hash)
);

-- Reverse lookup "which playlists contain this hash" and the resolution join.
create index playlist_tracks_hash on playlist_tracks (hash);