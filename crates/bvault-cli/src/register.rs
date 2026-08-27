use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Password};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

use crate::client::{get_api_url, Session};

#[derive(Serialize)]
struct RegisterRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct RegisterResponse {
    token: String,
    user_id: Uuid,
}

pub async fn run_register_flow() -> Result<()> {
    let username: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Choose a Username")
        .interact_text()?;

    let password = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Choose a Password (min 8 chars)")
        .interact()?;

    let base_url = get_api_url();
    let http = Client::new();

    let res = http
        .post(format!("{}/auth/register", base_url))
        .json(&RegisterRequest {
            username: &username,
            password: &password,
        })
        .send()
        .await?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        anyhow::bail!("Registration failed: {}", err_text);
    }

    let register_data: RegisterResponse = res.json().await?;

    let session = Session {
        token: register_data.token,
        user_id: register_data.user_id,
    };

    let mut path = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("bvault");
    fs::create_dir_all(&path)?;
    path.push("session.json");

    let content = serde_json::to_string(&session)?;
    fs::write(path, content)?;

    println!("✓ Registered and logged in successfully, {}!", username);
    Ok(())
}
