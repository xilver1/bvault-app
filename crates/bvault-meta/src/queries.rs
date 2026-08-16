//! Metadata queries. This is the only component with schema knowledge of
//! playlists and the raw-file lookup; the analysis worker never sees it.
//!
//! Note the division of labour around "analyzed": [`Meta::resolve_hashes`]
//! returns *all* member hashes with their raw locations. It does **not** filter
//! out already-analyzed tracks — that filter is `ArtifactStore::exists`, applied
//! by the gateway, because analyzed-ness is a fact about the store, not the DB.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use uuid::Uuid;

use crate::model::{Batch, Playlist, Track};
use crate::{Error, Result};

/// Handle to the metadata database. Cheap to clone — wraps the pool.
#[derive(Clone)]
pub struct Meta {
    pool: PgPool,
}

impl Meta {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ---- auth: users + sessions --------------------------------------------

    pub async fn create_user(&self, username: &str, password_hash: &str) -> Result<Uuid> {
        match sqlx::query_as::<_, (Uuid,)>(
            "insert into users (username, password_hash) values ($1, $2) returning id",
        )
        .bind(username)
        .bind(password_hash)
        .fetch_one(&self.pool)
        .await
        {
            Ok((id,)) => Ok(id),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(Error::UsernameTaken),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn find_credentials(&self, username: &str) -> Result<Option<(Uuid, String)>> {
        Ok(sqlx::query_as::<_, (Uuid, String)>(
            "select id, password_hash from users where lower(username) = lower($1)",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn create_session(
        &self,
        token_hash: &str,
        user_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("insert into sessions (token_hash, user_id, expires_at) values ($1, $2, $3)")
            .bind(token_hash)
            .bind(user_id)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn lookup_session(&self, token_hash: &str) -> Result<Option<Uuid>> {
        Ok(sqlx::query_as::<_, (Uuid,)>(
            "select user_id from sessions where token_hash = $1 and expires_at > now()",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?
        .map(|(id,)| id))
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<()> {
        sqlx::query("delete from sessions where token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- metadata operations ------------------------------------------------

    pub async fn upsert_track(
        &self,
        user_id: Option<Uuid>,
        hash: &str,
        raw_location: &str,
        title: Option<&str>,
        artist: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into tracks (hash, raw_location, title, artist, user_id)
            values ($1, $2, $3, $4, $5)
            on conflict (hash) do update
                set raw_location = excluded.raw_location,
                    title = excluded.title,
                    artist = excluded.artist,
                    user_id = coalesce(tracks.user_id, excluded.user_id)
            "#,
        )
        .bind(hash)
        .bind(raw_location)
        .bind(title)
        .bind(artist)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_tracks(&self, user_id: Option<Uuid>, limit: i64, offset: i64) -> Result<Vec<Track>> {
        Ok(sqlx::query_as::<_, Track>(
            r#"
            select hash, raw_location, title, artist, added_at, user_id
            from tracks
            where ($1::uuid is null or user_id = $1)
            order by added_at desc
            limit $2 offset $3
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn create_playlist(
        &self,
        user_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        hashes: &[String],
    ) -> Result<Uuid> {
        let mut tx = self.pool.begin().await?;
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "select id from playlists where user_id = $1 and name = $2 limit 1"
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;

        let id = match existing {
            Some((existing_id,)) => existing_id,
            None => {
                let (new_id,): (Uuid,) = sqlx::query_as(
                    "insert into playlists (user_id, name, description) values ($1, $2, $3) returning id"
                )
                .bind(user_id)
                .bind(name)
                .bind(description)
                .fetch_one(&mut *tx)
                .await?;
                new_id
            }
        };

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

    pub async fn list_playlists(&self, user_id: Option<Uuid>) -> Result<Vec<Playlist>> {
        Ok(sqlx::query_as::<_, Playlist>(
            "select id, name, description, created_at, updated_at, user_id
             from playlists where ($1::uuid is null or user_id = $1) order by created_at desc",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_playlist(&self, user_id: Option<Uuid>, id: Uuid) -> Result<Option<Playlist>> {
        Ok(sqlx::query_as::<_, Playlist>(
            "select id, name, description, created_at, updated_at, user_id
             from playlists where id = $1 and ($2::uuid is null or user_id = $2)",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn playlist_hashes(&self, user_id: Option<Uuid>, id: Uuid) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as(
                "select pt.hash from playlist_tracks pt
                 join playlists p on p.id = pt.playlist_id
                 where pt.playlist_id = $1 and ($2::uuid is null or p.user_id = $2)",
            )
            .bind(id)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(h,)| h).collect())
    }

    pub async fn resolve_hashes(&self, user_id: Option<Uuid>, playlist_ids: &[Uuid]) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            r#"
            select distinct t.hash, t.raw_location
            from playlist_tracks pt
            join playlists p on p.id = pt.playlist_id
            join tracks t on t.hash = pt.hash
            where pt.playlist_id = any($1) and ($2::uuid is null or p.user_id = $2)
            "#,
        )
        .bind(playlist_ids)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn create_batch(&self, user_id: Option<Uuid>, name: Option<&str>, hashes: &[String]) -> Result<Uuid> {
        let (id,): (Uuid,) =
            sqlx::query_as("insert into batches (user_id, name, hashes) values ($1, $2, $3) returning id")
                .bind(user_id)
                .bind(name)
                .bind(hashes)
                .fetch_one(&self.pool)
                .await?;
        Ok(id)
    }

    pub async fn get_batch(&self, user_id: Option<Uuid>, id: Uuid) -> Result<Option<Batch>> {
        Ok(sqlx::query_as::<_, Batch>(
            "select id, name, hashes, created_at, user_id from batches where id = $1 and ($2::uuid is null or user_id = $2)",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?)
    }
}