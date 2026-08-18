use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: Uuid,
}

pub fn get_config_dir() -> std::path::PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("bvault");
    path
}

pub fn load_session() -> Result<Session> {
    let mut path = get_config_dir();
    path.push("session.json");
    
    let content = fs::read_to_string(path)?;
    let session = serde_json::from_str(&content)?;
    Ok(session)
}

pub fn get_api_url() -> String {
    "http://192.168.0.200.nip.io".to_string()
}
