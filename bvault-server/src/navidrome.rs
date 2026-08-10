//! Navidrome/Subsonic API client for playlist integration
//!
//! Uses the Subsonic API to fetch playlists from Navidrome.
//! Authentication: token = MD5(password + salt)
//!
//! Reference: https://www.subsonic.org/pages/api.jsp

use std::collections::HashMap;
use md5::{Md5, Digest};
use rand::Rng;
use serde::Deserialize;
use tracing::{debug, info, warn};

/// Subsonic API client for Navidrome
pub struct NavidromeClient {
    base_url: String,
    username: String,
    password: String,
    client: reqwest::Client,
}

/// Playlist metadata from Navidrome
#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    pub duration_secs: u32,
    pub owner: String,
}

/// Track info from a playlist
#[derive(Debug, Clone)]
pub struct PlaylistTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: u32,
    /// Path relative to music library root
    pub path: String,
}

// Subsonic API response structures
#[derive(Deserialize)]
struct SubsonicResponse {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponseInner,
}

#[derive(Deserialize)]
struct SubsonicResponseInner {
    status: String,
    error: Option<SubsonicError>,
    playlists: Option<PlaylistsWrapper>,
    playlist: Option<PlaylistResponse>,
}

#[derive(Deserialize)]
struct SubsonicError {
    code: u32,
    message: String,
}

#[derive(Deserialize)]
struct PlaylistsWrapper {
    playlist: Option<PlaylistOrList>,
}

// Handle both single playlist and array of playlists (Subsonic API quirk)
#[derive(Deserialize)]
#[serde(untagged)]
enum PlaylistOrList {
    Single(PlaylistResponse),
    List(Vec<PlaylistResponse>),
}

#[derive(Deserialize)]
struct PlaylistResponse {
    id: String,
    name: String,
    #[serde(rename = "songCount", default)]
    song_count: u32,
    #[serde(default)]
    duration: u32,
    #[serde(default)]
    owner: String,
    entry: Option<EntryOrList>,
}

// Handle both single entry and array of entries
#[derive(Deserialize)]
#[serde(untagged)]
enum EntryOrList {
    Single(TrackEntry),
    List(Vec<TrackEntry>),
}

#[derive(Deserialize)]
struct TrackEntry {
    id: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    #[serde(default)]
    duration: u32,
    path: Option<String>,
}

impl NavidromeClient {
    /// Create a new Navidrome client
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();

        Self {
            base_url,
            username: username.to_string(),
            password: password.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Generate authentication parameters for Subsonic API
    fn auth_params(&self) -> HashMap<String, String> {
        // Generate random salt
        let salt: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(12)
            .map(char::from)
            .collect();

        // Calculate token = MD5(password + salt)
        let mut hasher = Md5::new();
        hasher.update(format!("{}{}", self.password, salt));
        let token = format!("{:x}", hasher.finalize());

        let mut params = HashMap::new();
        params.insert("u".to_string(), self.username.clone());
        params.insert("t".to_string(), token);
        params.insert("s".to_string(), salt);
        params.insert("v".to_string(), "1.16.0".to_string());
        params.insert("c".to_string(), "bvault".to_string());
        params.insert("f".to_string(), "json".to_string());
        params
    }

    /// Test connection to Navidrome
    pub async fn ping(&self) -> anyhow::Result<bool> {
        let url = format!("{}/rest/ping", self.base_url);
        let params = self.auth_params();

        let response = self.client
            .get(&url)
            .query(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            return Ok(false);
        }

        let body: SubsonicResponse = response.json().await?;
        Ok(body.response.status == "ok")
    }

    /// Get all playlists from Navidrome
    pub async fn get_playlists(&self) -> anyhow::Result<Vec<Playlist>> {
        let url = format!("{}/rest/getPlaylists", self.base_url);
        let params = self.auth_params();

        debug!("Fetching playlists from {}", url);

        let response = self.client
            .get(&url)
            .query(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch playlists: HTTP {}", response.status());
        }

        let body: SubsonicResponse = response.json().await?;

        if body.response.status != "ok" {
            if let Some(err) = body.response.error {
                anyhow::bail!("Subsonic error {}: {}", err.code, err.message);
            }
            anyhow::bail!("Unknown Subsonic error");
        }

        let playlists = match body.response.playlists {
            Some(wrapper) => match wrapper.playlist {
                Some(PlaylistOrList::Single(p)) => vec![p],
                Some(PlaylistOrList::List(list)) => list,
                None => vec![],
            },
            None => vec![],
        };

        let result: Vec<Playlist> = playlists
            .into_iter()
            .map(|p| Playlist {
                id: p.id,
                name: p.name,
                song_count: p.song_count,
                duration_secs: p.duration,
                owner: p.owner,
            })
            .collect();

        info!("Found {} playlists", result.len());
        Ok(result)
    }

    /// Get tracks from a specific playlist
    pub async fn get_playlist_tracks(&self, playlist_id: &str) -> anyhow::Result<Vec<PlaylistTrack>> {
        let url = format!("{}/rest/getPlaylist", self.base_url);
        let mut params = self.auth_params();
        params.insert("id".to_string(), playlist_id.to_string());

        debug!("Fetching playlist {} from {}", playlist_id, url);

        let response = self.client
            .get(&url)
            .query(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch playlist: HTTP {}", response.status());
        }

        let body: SubsonicResponse = response.json().await?;

        if body.response.status != "ok" {
            if let Some(err) = body.response.error {
                anyhow::bail!("Subsonic error {}: {}", err.code, err.message);
            }
            anyhow::bail!("Unknown Subsonic error");
        }

        let playlist = body.response.playlist
            .ok_or_else(|| anyhow::anyhow!("No playlist in response"))?;

        let entries = match playlist.entry {
            Some(EntryOrList::Single(e)) => vec![e],
            Some(EntryOrList::List(list)) => list,
            None => vec![],
        };

        let tracks: Vec<PlaylistTrack> = entries
            .into_iter()
            .filter_map(|e| {
                let path = e.path?;
                Some(PlaylistTrack {
                    id: e.id,
                    title: e.title.unwrap_or_else(|| "Unknown".to_string()),
                    artist: e.artist.unwrap_or_else(|| "Unknown".to_string()),
                    album: e.album,
                    duration_secs: e.duration,
                    path,
                })
            })
            .collect();

        debug!("Playlist {} has {} tracks", playlist_id, tracks.len());
        Ok(tracks)
    }

    /// Get all playlists with their tracks
    pub async fn get_all_playlist_tracks(&self) -> anyhow::Result<HashMap<String, Vec<PlaylistTrack>>> {
        let playlists = self.get_playlists().await?;
        let mut result = HashMap::new();

        for playlist in playlists {
            match self.get_playlist_tracks(&playlist.id).await {
                Ok(tracks) => {
                    info!("Loaded playlist '{}' with {} tracks", playlist.name, tracks.len());
                    result.insert(playlist.name, tracks);
                }
                Err(e) => {
                    warn!("Failed to load playlist '{}': {}", playlist.name, e);
                }
            }
        }

        Ok(result)
    }
}

/// Build a mapping from file paths to playlist names
///
/// This allows the analyzer to look up which playlist a track belongs to
/// based on its file path.
pub fn build_path_to_playlist_map(
    playlists: &HashMap<String, Vec<PlaylistTrack>>,
) -> HashMap<String, String> {
    let mut path_map = HashMap::new();

    for (playlist_name, tracks) in playlists {
        for track in tracks {
            // Normalize path separators
            let normalized_path = track.path.replace('\\', "/");
            path_map.insert(normalized_path, playlist_name.clone());
        }
    }

    path_map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_params() {
        let client = NavidromeClient::new(
            "http://localhost:4533",
            "admin",
            "password123",
        );

        let params = client.auth_params();

        assert_eq!(params.get("u"), Some(&"admin".to_string()));
        assert_eq!(params.get("v"), Some(&"1.16.0".to_string()));
        assert_eq!(params.get("c"), Some(&"rekordbox-export".to_string()));
        assert_eq!(params.get("f"), Some(&"json".to_string()));

        // Token and salt should be present
        assert!(params.contains_key("t"));
        assert!(params.contains_key("s"));

        // Salt should be 12 chars
        assert_eq!(params.get("s").unwrap().len(), 12);

        // Token should be 32 chars (MD5 hex)
        assert_eq!(params.get("t").unwrap().len(), 32);
    }

    #[test]
    fn test_path_to_playlist_map() {
        let mut playlists = HashMap::new();
        playlists.insert(
            "House".to_string(),
            vec![
                PlaylistTrack {
                    id: "1".to_string(),
                    title: "Track 1".to_string(),
                    artist: "Artist 1".to_string(),
                    album: None,
                    duration_secs: 300,
                    path: "Music/House/track1.mp3".to_string(),
                },
            ],
        );
        playlists.insert(
            "Techno".to_string(),
            vec![
                PlaylistTrack {
                    id: "2".to_string(),
                    title: "Track 2".to_string(),
                    artist: "Artist 2".to_string(),
                    album: None,
                    duration_secs: 360,
                    path: "Music/Techno/track2.flac".to_string(),
                },
            ],
        );

        let path_map = build_path_to_playlist_map(&playlists);

        assert_eq!(path_map.get("Music/House/track1.mp3"), Some(&"House".to_string()));
        assert_eq!(path_map.get("Music/Techno/track2.flac"), Some(&"Techno".to_string()));
    }
}
