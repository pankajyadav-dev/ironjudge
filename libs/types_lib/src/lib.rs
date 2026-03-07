use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TaskPayload {
    pub tasktype: TaskType,
    pub code: String,
    pub testcases: Vec<TestCaseType>,
    #[serde(default = "default_time")]
    pub timelimit: usize,
    #[serde(default = "default_memory")]
    pub memorylimit: usize,
    pub language: LanguageType,
}

#[derive(Debug)]
pub struct SandboxResult {
    pub exit_code: i32,
    pub signal: Option<i32>,
    pub wall_time_ms: u128,
}

pub type SandboxError = String;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SandboxConfiguration {
    pub submissionid: String,
    pub root_dir: PathBuf,
    pub input_file: PathBuf,
    pub output_file: PathBuf,
    pub user_output: PathBuf,
    pub error_output: PathBuf,
    pub time_limit: usize,
    pub memory_limit: usize,
    pub run_cmd_exe: String,
    pub run_cmd_args: Vec<String>,
}

impl SandboxConfiguration {
    pub fn process(
        submissionid: String,
        root_dir: PathBuf,
        input_file: PathBuf,
        output_file: PathBuf,
        user_output: PathBuf,
        error_output: PathBuf,
        time_limit: usize,
        memory_limit: usize,
        run_cmd_exe: String,
        run_cmd_args: Vec<String>,
    ) -> Self {
        Self {
            submissionid,
            root_dir,
            input_file,
            output_file,
            user_output,
            error_output,
            time_limit,
            memory_limit,
            run_cmd_exe,
            run_cmd_args,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TestCaseType {
    pub id: i32,
    pub input: String,
    pub output: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SubmissionIdPayload {
    pub submissionid: String,
}

impl SubmissionIdPayload {
    pub fn success(id: String) -> Self {
        Self { submissionid: id }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ResponsePayload {
    pub status: StatusType,
    pub message: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub ttpassed: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failedcase: Option<String>,
}

impl ResponsePayload {
    pub fn processing() -> Self {
        Self {
            status: StatusType::Processing,
            message: MessageType::Processing,
            error: None,
            ttpassed: 0,
            stdout: None,
            failedcase: None,
        }
    }
    pub fn error() -> Self {
        Self {
            status: StatusType::Error,
            message: MessageType::Error,
            error: None,
            ttpassed: 0,
            stdout: None,
            failedcase: None,
        }
    }

    pub fn success(stdout: Option<String>, tests: u32) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::Success,
            error: None,
            ttpassed: tests,
            stdout: stdout,
            failedcase: None,
        }
    }

    pub fn compiler_error(err: Option<String>) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::CompileTimeError,
            error: err,
            ttpassed: 0,
            stdout: None,
            failedcase: None,
        }
    }

    pub fn runtime_error(
        err: Option<String>,
        testpassed: u32,
        stdoutput: Option<String>,
        failedcase: Option<String>,
    ) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::RunTimeError,
            error: err,
            ttpassed: testpassed,
            stdout: stdoutput,
            failedcase: failedcase,
        }
    }

    pub fn time_error(tests: u32) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::TimeLimitError,
            error: Some("Time Limit Exceeded".to_string()),
            ttpassed: tests,
            stdout: None,
            failedcase: None,
        }
    }
    pub fn memory_error(tests: u32) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::MemoryLimitError,
            error: Some("Memory Limit Exceeded".to_string()),
            ttpassed: tests,
            stdout: None,
            failedcase: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LanguageType {
    Cpp,
    Java,
    Rust,
    Js,
    Ts,
    Py,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StatusType {
    Pending,
    Processing,
    Completed,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Processing,
    Success,
    CompileTimeError,
    RunTimeError,
    MemoryLimitError,
    TimeLimitError,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Run,
    Test,
}

#[derive(Clone, Debug)]
pub struct LanguageConfig {
    pub source_filename: &'static str,
    pub compile_cmd: Option<(&'static str, Vec<&'static str>)>,
    pub run_cmd: (&'static str, Vec<&'static str>),
}

impl LanguageConfig {
    pub fn get(language: &LanguageType) -> Self {
        // Resolve binary paths at runtime so execve inside the sandbox works.
        // Compilation runs on the host (tokio::process::Command searches PATH),
        // but sandbox.run() uses execve which requires absolute paths.
        match language {
            LanguageType::Cpp => LanguageConfig {
                source_filename: "main.cpp",
                compile_cmd: Some(("g++", vec!["main.cpp", "-O2", "-o", "solution"])),
                run_cmd: ("./solution", vec![]),
            },
            LanguageType::Rust => LanguageConfig {
                source_filename: "main.rs",
                compile_cmd: Some(("rustc", vec!["main.rs", "-O", "-o", "solution"])),
                run_cmd: ("./solution", vec![]),
            },
            LanguageType::Java => LanguageConfig {
                source_filename: "Main.java",
                compile_cmd: Some(("javac", vec!["Main.java"])),
                run_cmd: ("java", vec!["Main"]),
            },
            LanguageType::Py => LanguageConfig {
                source_filename: "solution.py",
                compile_cmd: None,
                run_cmd: ("python3", vec!["solution.py"]),
            },
            LanguageType::Js => LanguageConfig {
                source_filename: "solution.js",
                compile_cmd: None,
                run_cmd: ("bun", vec!["solution.js"]),
            },
            LanguageType::Ts => LanguageConfig {
                source_filename: "solution.ts",
                compile_cmd: None,
                run_cmd: ("bun", vec!["solution.ts"]),
            },
        }
    }
}

fn default_time() -> usize {
    2000
}
fn default_memory() -> usize {
    256
}
