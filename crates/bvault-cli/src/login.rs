use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Input, Password};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

use crate::client::{get_api_url, Session};

#[derive(Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
    user_id: Uuid,
}

pub async fn run_login_flow() -> Result<()> {
    let username: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Username")
        .interact_text()?;

    let password = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Password")
        .interact()?;

    let base_url = get_api_url();
    let http = Client::new();

    let res = http
        .post(&format!("{}/auth/login", base_url))
        .json(&LoginRequest {
            username: &username,
            password: &password,
        })
        .send()
        .await?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        anyhow::bail!("Login failed: {}", err_text);
    }

    let login_data: LoginResponse = res.json().await?;

    let session = Session {
        token: login_data.token,
        user_id: login_data.user_id,
    };

    let mut path = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("bvault");
    fs::create_dir_all(&path)?;
    path.push("session.json");

    let content = serde_json::to_string(&session)?;
    fs::write(path, content)?;

    println!("✓ Welcome back, {}!", username);
    Ok(())
}
