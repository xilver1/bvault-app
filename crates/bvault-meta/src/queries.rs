//! Metadata queries. This is the only component with schema knowledge of
//! playlists and the raw-file lookup; the analysis worker never sees it.
//!
//! Note the division of labour around "analyzed": [`Meta::resolve_hashes`]
//! returns *all* member hashes with their raw locations. It does **not** filter
//! out already-analyzed tracks — that filter is `ArtifactStore::exists`, applied
//! by the gateway, because analyzed-ness is a fact about the store, not the DB.

use sqlx::postgres::PgPool;
use uuid::Uuid;

use crate::model::{Batch, Playlist, Track};
use crate::Result;

/// Handle to the metadata database. Cheap to clone — wraps the pool.
#[derive(Clone)]
pub struct Meta {
    pool: PgPool,
}

impl Meta {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Register or update an ingested track's raw-file lookup entry. Ingestion
    /// writes this; the gateway reads it.
    pub async fn upsert_track(
        &self,
        hash: &str,
        raw_location: &str,
        title: Option<&str>,
        artist: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into tracks (hash, raw_location, title, artist)
            values ($1, $2, $3, $4)
            on conflict (hash) do update
                set raw_location = excluded.raw_location,
                    title = excluded.title,
                    artist = excluded.artist
            "#,
        )
        .bind(hash)
        .bind(raw_location)
        .bind(title)
        .bind(artist)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List the library (most-recently-added first).
    pub async fn list_tracks(&self, limit: i64, offset: i64) -> Result<Vec<Track>> {
        Ok(sqlx::query_as::<_, Track>(
            r#"
            select hash, raw_location, title, artist, added_at
            from tracks
            order by added_at desc
            limit $1 offset $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Create a playlist and its membership in one transaction. `hashes` is the
    /// unordered member set; duplicates collapse.
    pub async fn create_playlist(
        &self,
        name: &str,
        description: Option<&str>,
        hashes: &[String],
    ) -> Result<Uuid> {
        let mut tx = self.pool.begin().await?;
        let (id,): (Uuid,) =
            sqlx::query_as("insert into playlists (name, description) values ($1, $2) returning id")
                .bind(name)
                .bind(description)
                .fetch_one(&mut *tx)
                .await?;

        for hash in hashes {
            sqlx::query(
                "insert into playlist_tracks (playlist_id, hash) values ($1, $2)
                 on conflict do nothing",
            )
            .bind(id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_playlists(&self) -> Result<Vec<Playlist>> {
        Ok(sqlx::query_as::<_, Playlist>(
            "select id, name, description, created_at, updated_at
             from playlists order by created_at desc",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_playlist(&self, id: Uuid) -> Result<Option<Playlist>> {
        Ok(sqlx::query_as::<_, Playlist>(
            "select id, name, description, created_at, updated_at
             from playlists where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The member hashes of one playlist.
    pub async fn playlist_hashes(&self, id: Uuid) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("select hash from playlist_tracks where playlist_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(h,)| h).collect())
    }

    /// Resolve a set of playlists to their **deduplicated** member hashes, each
    /// paired with its raw-file location — the input the gateway needs to build
    /// analysis jobs (after filtering already-analyzed hashes via the store).
    pub async fn resolve_hashes(&self, playlist_ids: &[Uuid]) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            r#"
            select distinct t.hash, t.raw_location
            from playlist_tracks pt
            join tracks t on t.hash = pt.hash
            where pt.playlist_id = any($1)
            "#,
        )
        .bind(playlist_ids)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Record a submission batch: a snapshot of its full member hash set (plus an
    /// optional name for the completion notification). Returns the batch id the
    /// client polls. Progress is computed on read, never stored here.
    pub async fn create_batch(&self, name: Option<&str>, hashes: &[String]) -> Result<Uuid> {
        let (id,): (Uuid,) =
            sqlx::query_as("insert into batches (name, hashes) values ($1, $2) returning id")
                .bind(name)
                .bind(hashes)
                .fetch_one(&self.pool)
                .await?;
        Ok(id)
    }

    /// Fetch a batch (name + hash-set snapshot) for the gateway to compute
    /// progress against the artifact store and the dead-letter.
    pub async fn get_batch(&self, id: Uuid) -> Result<Option<Batch>> {
        Ok(sqlx::query_as::<_, Batch>(
            "select id, name, hashes, created_at from batches where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }
}