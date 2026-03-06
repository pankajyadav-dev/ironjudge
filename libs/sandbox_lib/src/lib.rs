use anyhow::Error;
use std::thread::sleep;
use std::{sync::Arc, thread::available_parallelism};
use tempfile::{Builder, TempDir};
use tokio::{sync::Semaphore, time::Duration};
use tracing::info;
use types_lib::{LanguageConfig, ResponsePayload, TaskPayload, TaskType, TestCaseType};

pub fn get_heavy_tasks_threads() -> usize {
    let total_cores = available_parallelism().map(|n| n.get()).unwrap_or(4);

    if total_cores <= 2 { 1 } else { total_cores - 2 }
}

pub async fn create_temp_file(directory: &str) -> Result<TempDir, Error> {
    let ram_dir = Builder::new().prefix(directory).tempdir_in("/dev/shm")?;
    Ok(ram_dir)
}

pub fn testcase_parsing(payload: Vec<TestCaseType>) -> (String,String) {
            let mut input_data = format!("{}\n", payload.len());
            let mut expected_output_data = String::new();

            for tc in &payload {
                input_data.push_str(&tc.input);
                if !input_data.ends_with('\n') {
                    input_data.push('\n');
                }

                if !expected_output_data.ends_with('\n') {
                    expected_output_data.push('\n');
                }
            }
            (input_data,expected_output_data)
}

pub async fn execute_submissions_detached(
    tasks: Vec<(String, TaskPayload)>,
    concurrency_limiter: Arc<Semaphore>,
) {
    for (submission_id, payload) in tasks {
        let permit = concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .expect("Semaphore closed");

        tokio::spawn(async move {
            let temp_dir = match create_temp_file(&submission_id).await {
                Ok(dir) => dir,
                Err(e) => {
                    println!(
                        "Failed to create temp directory for {}: {}",
                        submission_id, e
                    );
                    drop(permit);
                    return;
                }
            };

            let language_config = LanguageConfig::get(&payload.language);
            let source_path = temp_dir.path().join(language_config.source_filename);
            tokio::fs::write(&source_path, &payload.code).await.unwrap();
            if let Some((compiler, args)) = &language_config.compile_cmd {
                let compile_status = tokio::process::Command::new(compiler)
                    .args(args)
                    .current_dir(temp_dir.path())
                    .status()
                    .await
                    .unwrap();

                if !compile_status.success() {
                    println!("Compilation failed for {}", submission_id);
                    drop(permit);
                    return;
                }
            }

            let (sub_id, response) = tokio::task::spawn_blocking(move || {
                println!("Starting sandbox execution for job: {}", submission_id);
                println!("payload sandbox execution for job: {:?}", payload);
                println!("temp directory name {:?}", temp_dir);
                // YOUR SANDBOX-RS LOGIC GOES HERE

                sleep(Duration::from_secs(10));
                let response = ResponsePayload::success(
                    Some("Output matched!".to_string()),
                    payload.testcases.len() as u32,
                );

                (submission_id, response)
            })
            .await
            .expect("Blocking task panicked");

            info!(
                "Job {} completed with status: {:?}",
                sub_id, response.status
            );
            drop(permit);
        });
    }
}
