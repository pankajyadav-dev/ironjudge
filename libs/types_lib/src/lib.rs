use serde::{Deserialize, Serialize};
use std::{path::PathBuf};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TaskPayload {
    #[serde(default = "default_tasktype")]
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
    pub is_oom: bool,
}
#[derive(Debug)]
pub struct CompileResult {
    pub success: bool,
    pub error: String
}

pub type SandboxError = String;

#[derive(Debug)]
pub struct SandboxConfiguration {
    pub submissionid: String,
    pub root_dir: PathBuf,
    pub input_file: std::fs::File,
    pub output_file: std::fs::File,
    pub user_output: std::fs::File,
    pub error_output: std::fs::File,
    pub time_limit: usize,
    pub memory_limit: usize,
    pub run_cmd_exe: String,
    pub run_cmd_args: Vec<String>,
}

impl SandboxConfiguration {
    pub fn process(
        submissionid: String,
        root_dir: PathBuf,
        input_file: std::fs::File,
        output_file: std::fs::File,
        user_output: std::fs::File,
        error_output: std::fs::File,
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

#[derive(Debug)]
pub struct CompileSandboxConfig {
    pub submissionid: String,
    pub root_dir: PathBuf,
    pub time_limit: usize,
    pub memory_limit: usize,
    pub run_cmd_exe: &'static str,
    pub run_cmd_args: Vec<&'static str>,
}

impl CompileSandboxConfig {
    pub fn process(
        submissionid: String,
        root_dir: PathBuf,
        // output_file: std::fs::File,
        // error_output: std::fs::File,
        time_limit: usize,
        memory_limit: usize,
        run_cmd_exe: &'static str,
        run_cmd_args: Vec<&'static str>,
    ) -> Self {
        Self {
            submissionid,
            root_dir,
            // output_file,
            // error_output,
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

/// For Run task type: original test case + actual result
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TestCaseResult {
    pub id: i32,
    pub input: String,
    pub output: String,
    pub result: String,
    pub success: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FailedTestDetail {
    pub id: i32,
    pub input: String,
    pub expected: String,
    pub actual: String,
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
    pub results: Option<String>,
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
            results: None,
            failedcase: None,
        }
    }
    pub fn test_failed(ttpassed: u32, failedcase: Option<String>, stdout: Option<String>) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::Testcasefailed,
            error: None,
            ttpassed,
            stdout,
            results: None,
            failedcase,
        }
    }
    pub fn error(err: Option<String>) -> Self {
        Self {
            status: StatusType::Error,
            message: MessageType::Error,
            error: err,
            ttpassed: 0,
            stdout: None,
            results: None,
            failedcase: None,
        }
    }

    pub fn success(stdout: Option<String>, results: Option<String>, tests: u32) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::Success,
            error: None,
            ttpassed: tests,
            stdout,
            results,
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
            results: None,
            failedcase: None,
        }
    }

    pub fn runtime_error(
        err: Option<String>,
        testpassed: u32,
        stdout: Option<String>,
        failedcase: Option<String>,
    ) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::RunTimeError,
            error: err,
            ttpassed: testpassed,
            stdout,
            results: None,
            failedcase,
        }
    }

    pub fn time_error(tests: u32, stdout: Option<String>) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::TimeLimitError,
            error: Some("Time Limit Exceeded".to_string()),
            ttpassed: tests,
            stdout,
            results: None,
            failedcase: None,
        }
    }
    pub fn memory_error(tests: u32, stdout: Option<String>) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::MemoryLimitError,
            error: Some("Memory Limit Exceeded".to_string()),
            ttpassed: tests,
            stdout,
            results: None,
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
    Testcasefailed,
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
                run_cmd: (
                    "java",
                    vec!["-Xmx128M", "-Xms128M", "-XX:+UseSerialGC", "Main"],
                ),
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

fn default_tasktype() -> TaskType {
    TaskType::Run
}
fn default_time() -> usize {
    2000
}
fn default_memory() -> usize {
    256
}
