use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TaskPayload {
    pub tasktype: TaskType,
    pub code: String,
    pub testcases: String,
    #[serde(default = "default_time")]
    pub timelimit: u32,
    #[serde(default = "default_memory")]
    pub memorylimit: u32,
    pub language: LanguageType,
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

    pub fn success(stdout: String, tests: u32) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::Success,
            error: None,
            ttpassed: tests,
            stdout: Some(stdout),
            failedcase: None,
        }
    }

    pub fn compiler_error(err: String) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::CompileTimeError,
            error: Some(err),
            ttpassed: 0,
            stdout: None,
            failedcase: None,
        }
    }

    pub fn runtime_error(
        err: String,
        testpassed: u32,
        stdoutput: String,
        failedcase: String,
    ) -> Self {
        Self {
            status: StatusType::Completed,
            message: MessageType::RunTimeError,
            error: Some(err),
            ttpassed: testpassed,
            stdout: Some(stdoutput),
            failedcase: Some(failedcase),
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
