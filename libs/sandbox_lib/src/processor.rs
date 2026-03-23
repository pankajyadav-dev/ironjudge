use crate::action::{
    create_temp_file, is_valid_uuid_v7, read_bounded_string, testcase_parsing, validate_test_cases,
};
use crate::compilesandbox::compile_sandbox_runner;
use crate::sandbox::sandbox_runner;
// use tracing::info;

use types_lib::{CompileSandboxConfig, SandboxConfiguration};
use types_lib::{LanguageConfig, ResponsePayload, TaskPayload};

pub async fn process_single_submission(
    submission_id: &str,
    payload: &TaskPayload,
) -> anyhow::Result<ResponsePayload> {
    let temp_dir = create_temp_file(submission_id).await?;
    let root_dir_path = temp_dir.path().to_path_buf();

    let language_config = LanguageConfig::get(&payload.language);
    let source_path = root_dir_path.join(language_config.source_filename);
    tokio::fs::write(&source_path, &payload.code).await?;

    if !is_valid_uuid_v7(submission_id) {
        return Ok(ResponsePayload::error(Some(
            format!("Invalid submission id").to_string(),
        )));
    }

    if let Some((compiler, args)) = &language_config.compile_cmd {
        let compiler_config = CompileSandboxConfig {
            submissionid: submission_id.to_string(),
            run_cmd_args: args.clone(),
            root_dir: root_dir_path.clone(),
            run_cmd_exe: compiler,
            memory_limit: 1024,
            time_limit: 5000,
        };
        let compilation_result = compile_sandbox_runner(compiler_config).await;
        match compilation_result {
            Ok(result) => {
                if !result.success {
                    return Ok(ResponsePayload::compiler_error(Some(result.error)));
                }
            }
            Err(e) => {
                return Ok(ResponsePayload::compiler_error(Some(
                    format!("Sandbox error: {}", e).to_string(),
                )));
            }
        }

        // if let Some((status, error)) = compilation_result {
        // if !status.success() {
        // return Ok(ResponsePayload::compiler_error(Some(error)));
        // }
        // }
        // compiler: compiler.to_string(),
        // args: args.clone(),
        // current_dir: root_dir_path,
        // let compile_result = tokio::process::Command::new(compiler)
        //     .args(args)
        //     .current_dir(&root_dir_path)
        //     .stderr(std::process::Stdio::piped())
        //     .output()
        //     .await?;

        // if !compile_result.status.success() {
        //     let stderr = String::from_utf8_lossy(&compile_result.stderr).to_string();
        //     info!("compilation failed for {}: {}", submission_id, stderr);
        // }
    }

    let (input_data, _expected_output) = testcase_parsing(payload.testcases.clone());
    let input_file_path = root_dir_path.join("input.txt");
    let output_file_path = root_dir_path.join("output.txt");
    let user_output_file_path = root_dir_path.join("user_output.txt");
    let error_file_path = root_dir_path.join("error.txt");

    tokio::fs::write(&input_file_path, input_data).await?;

    let in_file = tokio::fs::File::open(&input_file_path).await?.into_std();
    let out_file = tokio::fs::File::create(&output_file_path).await?.into_std();
    let err_file = tokio::fs::File::create(&error_file_path).await?.into_std();
    let user_output_file = tokio::fs::File::create(&user_output_file_path)
        .await?
        .into_std();

    let time_limit_millisec = std::cmp::max(1000, payload.timelimit);

    let sandbox_config = SandboxConfiguration {
        submissionid: submission_id.to_string(),
        memory_limit: payload.memorylimit,
        time_limit: time_limit_millisec,
        root_dir: root_dir_path.clone(),
        input_file: in_file.await,
        output_file: out_file.await,
        error_output: err_file.await,
        user_output: user_output_file.await,
        run_cmd_exe: language_config.run_cmd.0.to_string(),
        run_cmd_args: language_config
            .run_cmd
            .1
            .iter()
            .map(|&s| s.to_string())
            .collect(),
    };

    let sandbox_result = sandbox_runner(sandbox_config)
        .await
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let output_limit = 1024 * 512; // .5 MB

    // let fd3_string = read_bounded_string(&output_file_path, output_limit).await.unwrap_or_default();
    let user_stdout_raw = read_bounded_string(&user_output_file_path, output_limit)
        .await
        .unwrap_or_default();

    let fd3_string = tokio::fs::read_to_string(&output_file_path)
        .await
        .unwrap_or_default();
    let fd3_lines: Vec<String> = fd3_string
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    // let user_stdout_raw = tokio::fs::read_to_string(&user_output_file_path)
    // .await
    // .unwrap_or_default();
    let user_stdout = if user_stdout_raw.trim().is_empty() {
        None
    } else {
        Some(user_stdout_raw)
    };

    let effective_signal = sandbox_result.signal.or_else(|| {
        if sandbox_result.exit_code > 128 {
            Some(sandbox_result.exit_code - 128)
        } else {
            None
        }
    });

    let actual_error = read_bounded_string(&error_file_path, output_limit)
        .await
        .unwrap_or_default();
    // let actual_error = tokio::fs::read_to_string(&error_file_path)
    // .await
    // .unwrap_or_default();

    let response = if sandbox_result.is_oom {
        ResponsePayload::memory_error(0, user_stdout.clone())
    } else if let Some(signal) = effective_signal {
        let error_msg = match signal {
            11 => "runtime error: segmentation fault (sigsegv)".to_string(),
            24 => "time limit exceeded (cpu time)".to_string(),
            9 => {
                if sandbox_result.wall_time_ms >= payload.timelimit as u128 {
                    "time limit exceeded (wall time)".to_string()
                } else {
                    "memory limit exceeded / killed".to_string()
                }
            }
            8 => "runtime error: floating point exception".to_string(),
            6 => "runtime error: aborted (sigabrt)".to_string(),
            31 => "security violation: unauthorized system call blocked (sigsys)".to_string(),
            _ => format!("runtime error: killed by signal {}", signal),
        };

        match signal {
            24 => ResponsePayload::time_error(0, user_stdout.clone()),
            9 => {
                if sandbox_result.wall_time_ms >= payload.timelimit as u128 {
                    ResponsePayload::time_error(0, user_stdout.clone())
                } else {
                    ResponsePayload::memory_error(0, user_stdout.clone())
                }
            }
            _ => {
                let full_err_msg = if actual_error.is_empty() {
                    error_msg
                } else {
                    format!("{}\n{}", error_msg, actual_error)
                };
                ResponsePayload::runtime_error(Some(full_err_msg), 0, user_stdout.clone(), None)
            }
        }
    } else if sandbox_result.exit_code != 0 {
        let error_msg = format!("runtime error (exit code: {})", sandbox_result.exit_code);
        let full_err_msg = if actual_error.is_empty() {
            error_msg
        } else {
            format!("{}\n{}", error_msg, actual_error)
        };
        ResponsePayload::runtime_error(Some(full_err_msg), 0, user_stdout.clone(), None)
    } else {
        validate_test_cases(
            fd3_lines,
            &payload.testcases,
            &payload.tasktype,
            user_stdout.clone(),
        )
    };

    Ok(response)
}
