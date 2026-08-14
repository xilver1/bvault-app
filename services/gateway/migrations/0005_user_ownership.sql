-- User ownership and tenancy columns for tracks, playlists, and batches.

alter table tracks add column user_id uuid references users(id) on delete cascade;
alter table playlists add column user_id uuid references users(id) on delete cascade;
alter table batches add column user_id uuid references users(id) on delete cascade;

create index tracks_user_id on tracks (user_id);
create index playlists_user_id on playlists (user_id);
create index batches_user_id on batches (user_id);
