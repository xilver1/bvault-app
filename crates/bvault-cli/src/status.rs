use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

use crate::client::{get_api_url, load_session};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum JobKind {
    Analysis,
    YtDlpIngest,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Dead,
}

#[derive(Deserialize, Debug)]
struct QueueStatus {
    kind: JobKind,
    status: JobStatus,
    count: i64,
}

#[derive(Deserialize, Debug)]
struct StatusResponse {
    jobs: Vec<QueueStatus>,
}

pub async fn run_status_flow() -> Result<()> {
    let session = load_session()?;
    let base_url = get_api_url();
    let http = Client::new();

    let res = http
        .get(&format!("{}/status", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to get status: {}", err);
    }

    let status_data: StatusResponse = res.json().await?;

    println!("Background Job Status:");
    if status_data.jobs.is_empty() {
        println!("  No jobs currently recorded.");
        return Ok(());
    }

    for job in status_data.jobs {
        let kind = match job.kind {
            JobKind::Analysis => "Analysis",
            JobKind::YtDlpIngest => "YouTube Ingest",
        };
        let status = match job.status {
            JobStatus::Pending => "Pending",
            JobStatus::Running => "Running",
            JobStatus::Succeeded => "Succeeded",
            JobStatus::Failed => "Failed",
            JobStatus::Dead => "Dead (Permanently Failed)",
        };
        println!("  [{}] {}: {}", kind, status, job.count);
    }

    Ok(())
}
