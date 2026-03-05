use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TaskPayload {
    pub tasktype: TaskType,
    pub code: String,
    pub testcases: Vec<TestCaseType>,
    #[serde(default = "default_time")]
    pub timelimit: u32,
    #[serde(default = "default_memory")]
    pub memorylimit: u32,
    pub language: LanguageType,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TestCaseType{
    pub id: i32,
    pub input: String,
    pub output: String
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

fn default_time() -> u32 {
    2000
}
fn default_memory() -> u32 {
    256
}
