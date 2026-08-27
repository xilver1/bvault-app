use anyhow::Result;
use reqwest::Client;
use std::fs;

use crate::client::{get_api_url, load_session};

pub async fn run_logout_flow() -> Result<()> {
    let session = match load_session() {
        Ok(s) => s,
        Err(_) => {
            println!("You are not currently logged in.");
            return Ok(());
        }
    };

    let base_url = get_api_url();
    let http = Client::new();

    let _res = http
        .post(format!("{}/auth/logout", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await;

    let mut path = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("bvault");
    path.push("session.json");

    if path.exists() {
        fs::remove_file(path)?;
    }

    println!("✓ Successfully logged out.");
    Ok(())
}
