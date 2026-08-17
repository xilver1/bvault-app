-- 1. Create the many-to-many ownership table
create table user_tracks (
    user_id   uuid not null references users(id) on delete cascade,
    hash      text not null references tracks(hash) on delete cascade,
    added_at  timestamptz not null default now(),
    primary key (user_id, hash)
);

create index user_tracks_user_id on user_tracks (user_id);

-- 2. Migrate existing ownership data
insert into user_tracks (user_id, hash, added_at)
select user_id, hash, added_at
from tracks
where user_id is not null
on conflict do nothing;

-- 3. Drop the single-owner column from the global registry
alter table tracks drop column user_id;
