use std::sync::Arc;
use tracing::{info};
use tokio::{sync::Semaphore,time::Duration};
use std::thread::sleep;
use types_lib::{TaskPayload, ResponsePayload};

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
            let (sub_id, response) = tokio::task::spawn_blocking(move || {
                println!("Starting sandbox execution for job: {}", submission_id);
                
                
                
                // YOUR SANDBOX-RS LOGIC GOES HERE
                
                
                
                
                sleep(Duration::from_secs(10));
                let response = ResponsePayload::success(
                    Some("Output matched!".to_string()), 
                    payload.testcases.len() as u32
                );
                
                (submission_id, response)
            })
            .await 
            .expect("Blocking task panicked");

            info!("Job {} completed with status: {:?}", sub_id, response.status);
            drop(permit);
        });
    }
}