-- 1. Add title and artist to user_tracks
ALTER TABLE user_tracks
ADD COLUMN title TEXT,
ADD COLUMN artist TEXT;

-- 2. Migrate existing display metadata from tracks to user_tracks
UPDATE user_tracks ut
SET title = t.title,
    artist = t.artist
FROM tracks t
WHERE ut.hash = t.hash;

-- 3. Drop the global display metadata columns from tracks
ALTER TABLE tracks
DROP COLUMN title,
DROP COLUMN artist;
