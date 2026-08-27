use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

use crate::client::{get_api_url, get_config_dir, load_session};
use crate::StatusCommands;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

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

#[derive(Deserialize)]
struct JobStatusResponse {
    status: String,
}

#[derive(Deserialize)]
struct LastIngestState {
    job_ids: Vec<i64>,
}

pub async fn run_status_flow(command: Option<&StatusCommands>) -> Result<()> {
    match command {
        None => run_global_status().await,
        Some(StatusCommands::Ingest) => run_ingest_status().await,
        Some(StatusCommands::Analysis) => run_analysis_status().await,
    }
}

async fn run_global_status() -> Result<()> {
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

async fn run_ingest_status() -> Result<()> {
    let mut state_path = get_config_dir();
    state_path.push("last_ingest.json");

    let content = match fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(_) => {
            anyhow::bail!("No background ingest operation found.");
        }
    };

    let state: LastIngestState = serde_json::from_str(&content)?;
    let job_ids = state.job_ids;

    if job_ids.is_empty() {
        println!("Background ingest batch is empty.");
        return Ok(());
    }

    let session = load_session()?;
    let base_url = get_api_url();
    let http = Client::new();

    println!("Resuming tracking for {} background jobs...", job_ids.len());
    let pb_batch = ProgressBar::new(job_ids.len() as u64);
    pb_batch.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({msg})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut completed_jobs = std::collections::HashSet::new();
    let mut failed = 0;

    loop {
        if completed_jobs.len() == job_ids.len() {
            break;
        }
        let mut progress_made = false;
        for &jid in &job_ids {
            if completed_jobs.contains(&jid) {
                continue;
            }
            let res = http
                .get(format!("{}/jobs/{}", base_url, jid))
                .header("Authorization", format!("Bearer {}", session.token))
                .send()
                .await;
            if let Ok(r) = res {
                if let Ok(status) = r.json::<JobStatusResponse>().await {
                    match status.status.as_str() {
                        "succeeded" => {
                            completed_jobs.insert(jid);
                            pb_batch.inc(1);
                            progress_made = true;
                        }
                        "failed" | "dead" => {
                            completed_jobs.insert(jid);
                            failed += 1;
                            pb_batch.inc(1);
                            progress_made = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        pb_batch.set_message(format!("{} failed", failed));
        if !progress_made {
            sleep(Duration::from_secs(3)).await;
        }
    }
    pb_batch.finish_with_message(format!("Batch complete. {} failed.", failed));

    // Cleanup state file after completion
    let _ = fs::remove_file(state_path);
    Ok(())
}

async fn run_analysis_status() -> Result<()> {
    let session = load_session()?;
    let base_url = get_api_url();
    let http = Client::new();

    println!("Monitoring global analysis queue...");
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.magenta/blue}] {pos}/{len} ({msg})")
            .unwrap()
            .progress_chars("#>-"),
    );

    loop {
        let res = http
            .get(&format!("{}/status", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?;

        if !res.status().is_success() {
            anyhow::bail!("Failed to get status");
        }

        let status_data: StatusResponse = res.json().await?;
        let mut pending = 0;
        let mut running = 0;
        let mut succeeded = 0;
        let mut failed = 0;
        let mut dead = 0;

        for job in status_data.jobs {
            if let JobKind::Analysis = job.kind {
                match job.status {
                    JobStatus::Pending => pending += job.count,
                    JobStatus::Running => running += job.count,
                    JobStatus::Succeeded => succeeded += job.count,
                    JobStatus::Failed => failed += job.count,
                    JobStatus::Dead => dead += job.count,
                }
            }
        }

        let total = pending + running + succeeded + failed + dead;
        let done = succeeded + dead;

        if total == 0 {
            pb.finish_with_message("No analysis jobs in queue.");
            break;
        }

        pb.set_length(total as u64);
        pb.set_position(done as u64);
        pb.set_message(format!(
            "{} pending/running, {} failed",
            pending + running,
            failed + dead
        ));

        if done >= total {
            pb.finish_with_message(format!(
                "Analysis queue complete. {} failed/dead.",
                failed + dead
            ));
            break;
        }

        sleep(Duration::from_secs(3)).await;
    }

    Ok(())
}
