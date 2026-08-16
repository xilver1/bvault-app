-- Track yt-dlp ingest runs in the existing job table so the CLI polls terminal
-- state instead of diffing /tracks. Download runs in the Python service; the
-- gateway owns the row and flips it succeeded/dead via an internal callback.
--
-- PG12+ allows ADD VALUE inside a transaction provided the value isn't *used*
-- in the same txn (it isn't), so sqlx's per-migration transaction is fine.
alter type job_kind add value if not exists 'yt_dlp_ingest';