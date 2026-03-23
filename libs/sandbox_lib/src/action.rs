use anyhow::Error;
use std::thread::available_parallelism;
use tempfile::{Builder, TempDir};
use tokio::io::AsyncReadExt;
use types_lib::{FailedTestDetail, ResponsePayload, TaskType, TestCaseResult, TestCaseType};
use uuid::Uuid;
// use tracing::info;

pub fn is_valid_uuid_v7(input: &str) -> bool {
    match Uuid::parse_str(input) {
        Ok(parsed_uuid) => {
            // info!("Parsed UUID: {}", parsed_uuid);
            // info!("Parsed UUID VERSION: {}", parsed_uuid.get_version_num());
            parsed_uuid.get_version_num() == 7
        }
        Err(_) => false,
    }
}

pub async fn read_bounded_string(
    path: &std::path::Path,
    max_bytes: u64,
) -> std::io::Result<String> {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return Ok(String::new()),
    };

    let mut buffer = String::new();
    file.take(max_bytes).read_to_string(&mut buffer).await?;

    if let Ok(metadata) = tokio::fs::metadata(path).await {
        if metadata.len() > max_bytes {
            buffer.push_str("\n... [Output Truncated: Exceeded 1MB limit]");
        }
    }

    Ok(buffer)
}

pub fn get_heavy_tasks_threads() -> usize {
    let total_cores = available_parallelism().map(|n| n.get()).unwrap_or(4);
    match total_cores {
        1..=2 => 1,
        3..=4 => total_cores - 1,
        _ => total_cores - 2,
    }
}

pub async fn create_temp_file(directory: &str) -> Result<TempDir, Error> {
    let ram_dir = Builder::new().prefix(directory).tempdir_in("/dev/shm")?;
    Ok(ram_dir)
}

pub fn testcase_parsing(payload: Vec<TestCaseType>) -> (String, Vec<String>) {
    let mut input_data = format!("{}\n", payload.len());
    let mut expected_output_data = Vec::new();

    for tc in &payload {
        input_data.push_str(&tc.input);
        if !input_data.ends_with('\n') {
            input_data.push('\n');
        }
        for line in tc.output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                expected_output_data.push(trimmed.to_string());
            }
        }
    }
    (input_data, expected_output_data)
}

pub fn validate_test_cases(
    fd3_output: Vec<String>,
    testcases: &[TestCaseType],
    tasktype: &TaskType,
    user_stdout: Option<String>,
) -> ResponsePayload {
    match tasktype {
        TaskType::Run => {
            let results: Vec<TestCaseResult> = testcases
                .iter()
                .enumerate()
                .map(|(i, tc)| {
                    let result = fd3_output.get(i).cloned().unwrap_or_default();
                    TestCaseResult {
                        id: tc.id,
                        input: tc.input.trim().to_string(),
                        output: tc.output.trim().to_string(),
                        result: result.clone(),
                        success: tc.output.trim().eq(&result),
                    }
                })
                .collect();

            let results_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
            ResponsePayload::success(user_stdout, Some(results_json), testcases.len() as u32)
        }
        TaskType::Test => {
            if testcases.is_empty() {
                return ResponsePayload::success(user_stdout, None, 0);
            }

            let expected: Vec<String> = testcases
                .iter()
                .flat_map(|tc| {
                    tc.output
                        .lines()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                })
                .collect();

            let total = expected.len();
            let mut passed: u32 = 0;

            for (i, exp) in expected.iter().enumerate() {
                let actual = fd3_output.get(i).map(|s| s.as_str()).unwrap_or("");
                if actual != exp.as_str() {
                    let mut line_cursor = 0;
                    let mut failed_tc = testcases.first().unwrap();

                    for tc in testcases {
                        let tc_line_count =
                            tc.output.lines().filter(|l| !l.trim().is_empty()).count();
                        if i < line_cursor + tc_line_count {
                            failed_tc = tc;
                            break;
                        }
                        line_cursor += tc_line_count;
                    }

                    let detail = FailedTestDetail {
                        id: failed_tc.id,
                        input: failed_tc.input.trim().to_string(),
                        expected: exp.clone(),
                        actual: actual.to_string(),
                    };
                    let detail_json =
                        serde_json::to_string(&detail).unwrap_or_else(|_| "{}".to_string());
                    return ResponsePayload::test_failed(passed, Some(detail_json), user_stdout);
                }
                passed += 1;
            }

            if fd3_output.len() > expected.len() {
                let failed_tc = testcases.last().unwrap();
                let detail = FailedTestDetail {
                    id: failed_tc.id,
                    input: failed_tc.input.trim().to_string(),
                    expected: "EOF (no more output expected)".to_string(),
                    actual: fd3_output[expected.len()].clone(),
                };
                let detail_json =
                    serde_json::to_string(&detail).unwrap_or_else(|_| "{}".to_string());
                return ResponsePayload::test_failed(passed, Some(detail_json), user_stdout);
            }

            ResponsePayload::success(user_stdout, None, total as u32)
        }
    }
}
