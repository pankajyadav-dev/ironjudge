# IronJudge — API Contract

Base URL: `http://<host>:<port>`

---

## 1. Health Check

```
GET /
```

**Response** `200 OK`

```json
"the service is healthy"
```

---

## 2. Submit Code (Run Mode)

Executes the code against every test case and returns the **actual output** for each one — no pass/fail comparison.

```
POST /run
Content-Type: application/json
Headers: "x-user-id" : userid(to rate limit the submiision and polling)
```

### Request Body

| Field         | Type         | Required | Default | Description                                     |
| ------------- | ------------ | -------- | ------- | ----------------------------------------------- |
| `code`        | `string`     | ✅       |         | Source code to execute                          |
| `language`    | `string`     | ✅       |         | One of: `cpp`, `java`, `rust`, `js`, `ts`, `py` |
| `testcases`   | `TestCase[]` | ✅       |         | Array of test cases (see below)                 |
| `timelimit`   | `number`     | ❌       | `2000`  | Time limit in **milliseconds**                  |
| `memorylimit` | `number`     | ❌       | `256`   | Memory limit in **MB**                          |

**TestCase Object:**

| Field    | Type     | Description         |
| -------- | -------- | ------------------- |
| `id`     | `number` | Unique test case ID |
| `input`  | `string` | stdin input         |
| `output` | `string` | Expected output     |

### Example Request — Run mode

The submitted code must write test case answers to **fd3** (file descriptor 3) and can print debug/log output to **stdout** as usual.

```json
{
    "code": "import * as fs from 'fs';\nconst input = fs.readFileSync(0, 'utf-8').trim().split(/\\s+/);\nlet ptr = 0;\nconst t = parseInt(input[ptr++], 10);\nconsole.log(`Processing ${t} test cases...`);\nfor (let i = 0; i < t; i++) {\n    const a = parseInt(input[ptr++], 10);\n    const b = parseInt(input[ptr++], 10);\n    fs.writeSync(3, (a + b) + '\\n');\n}\nconsole.log('Done.');",
    "language": "ts",
    "testcases": [
        { "id": 1, "input": "5 7\n", "output": "12\n" },
        { "id": 2, "input": "10 20\n", "output": "30\n" },
        { "id": 3, "input": "1 1\n", "output": "2\n" },
        { "id": 4, "input": "99 1\n", "output": "100\n" },
        { "id": 5, "input": "-5 5\n", "output": "0\n" }
    ],
    "timelimit": 2000,
    "memorylimit": 256
}
```

> **Key**: test answers are written to `fd3` via `fs.writeSync(3, ...)`.  
> Regular `console.log()` goes to stdout and will appear in the `stdout` response field.

### Example Request — Test mode

Same payload shape, just send to `/test` instead of `/run`.

```json
{
    "code": "import * as fs from 'fs';\nconst input = fs.readFileSync(0, 'utf-8').trim().split(/\\s+/);\nlet ptr = 0;\nconst t = parseInt(input[ptr++], 10);\nconsole.log(`Running ${t} tests`);\nfor (let i = 0; i < t; i++) {\n    const a = parseInt(input[ptr++], 10);\n    const b = parseInt(input[ptr++], 10);\n    fs.writeSync(3, (a + b) + '\\n');\n}",
    "language": "ts",
    "testcases": [
        { "id": 1, "input": "5 7\n", "output": "12\n" },
        { "id": 2, "input": "10 20\n", "output": "30\n" },
        { "id": 3, "input": "1 1\n", "output": "2\n" }
    ],
    "timelimit": 2000,
    "memorylimit": 256
}
```

### Response `200 OK`

```json
{
    "submissionid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

---

## 4. Poll Submission Status

```
GET /status/{submissionid}
```

`submissionid` must be a valid **UUID** returned from `/run` or `/test`.

### Response `200 OK`

The response shape is always:

```json
{
    "status": "<status>",
    "message": "<message>",
    "ttpassed": 0,
    "error": "...",
    "stdout": "...",
    "results": "...",
    "failedcase": "..."
}
```

> Fields `error`, `stdout`, `results`, and `failedcase` are **omitted** when `null`.
>
> - `stdout` — user's console output (regular print/println), available in all response types
> - `results` — JSON array of test case results (Run mode only, see below)

---

### Status & Message Values

| `status`       | `message`              | Meaning                               |
| -------------- | ---------------------- | ------------------------------------- |
| `"pending"`    | `"processing"`         | Task is queued, not yet picked up     |
| `"processing"` | `"processing"`         | Worker is currently executing         |
| `"completed"`  | `"success"`            | All test cases passed / executed      |
| `"completed"`  | `"testcasefailed"`     | A test case failed (Test mode only)   |
| `"completed"`  | `"compile_time_error"` | Code failed to compile                |
| `"completed"`  | `"run_time_error"`     | Runtime crash (segfault, abort, etc.) |
| `"completed"`  | `"time_limit_error"`   | Execution exceeded time limit         |
| `"completed"`  | `"memory_limit_error"` | Execution exceeded memory limit       |
| `"error"`      | `"error"`              | Internal server error                 |

---

### Response Examples

#### Still processing

```json
{
    "status": "processing",
    "message": "processing",
    "ttpassed": 0
}
```

#### Success — Run mode

`results` contains a JSON array of test case results (answers written to **fd3**).
`stdout` contains the user's console output (regular `console.log` / `print` / `cout`).

```json
{
    "status": "completed",
    "message": "success",
    "ttpassed": 5,
    "stdout": "Processing 5 test cases...\nDone.",
    "results": "[{\"id\":1,\"input\":\"5 7\",\"output\":\"12\",\"result\":\"12\",\"success\":\"true\"},{\"id\":2,\"input\":\"10 20\",\"output\":\"30\",\"result\":\"30\",\"success\":\"true\"},{\"id\":3,\"input\":\"1 1\",\"output\":\"2\",\"result\":\"2\",\"success\":\"true\"},{\"id\":4,\"input\":\"99 1\",\"output\":\"100\",\"result\":\"100\",\"success\":\"true\"},{\"id\":5,\"input\":\"-5 5\",\"output\":\"0\",\"result\":\"0\",\"success\":\"true\"}]"
}
```

**Parsed `results` (TestCaseResult[]):**

| Field    | Type     | Description               |
| -------- | -------- | ------------------------- |
| `id`     | `number` | Test case ID              |
| `input`  | `string` | stdin input (trimmed)     |
| `output` | `string` | Expected output (trimmed) |
| `result` | `string` | Actual output from fd3    |

#### Success — Test mode

```json
{
    "status": "completed",
    "message": "success",
    "ttpassed": 3,
    "stdout": "Running 3 tests"
}
```

> `stdout` is omitted if the code didn't print anything to console.

#### Test case failed — Test mode

`failedcase` contains JSON details of the **first** failing test case.
`stdout` still carries any console output the code produced before/during execution.

```json
{
    "status": "completed",
    "message": "testcasefailed",
    "ttpassed": 1,
    "stdout": "Running 3 tests",
    "failedcase": "{\"id\":2,\"input\":\"10 20\",\"expected\":\"30\",\"actual\":\"31\"}"
}
```

**Parsed `failedcase` (FailedTestDetail):**

| Field      | Type     | Description          |
| ---------- | -------- | -------------------- |
| `id`       | `number` | Failing test case ID |
| `input`    | `string` | stdin input          |
| `expected` | `string` | Expected output line |
| `actual`   | `string` | Actual output line   |

#### Compilation error

```json
{
    "status": "completed",
    "message": "compile_time_error",
    "ttpassed": 0,
    "error": "main.cpp:3:1: error: 'asdf' was not declared in this scope"
}
```

#### Runtime error

```json
{
    "status": "completed",
    "message": "run_time_error",
    "ttpassed": 0,
    "error": "Runtime Error: Segmentation Fault (SIGSEGV)",
    "stdout": "partial output before crash..."
}
```

#### Time limit exceeded

```json
{
    "status": "completed",
    "message": "time_limit_error",
    "ttpassed": 0,
    "error": "Time Limit Exceeded",
    "stdout": "partial output before timeout..."
}
```

#### Memory limit exceeded

```json
{
    "status": "completed",
    "message": "memory_limit_error",
    "ttpassed": 0,
    "error": "Memory Limit Exceeded",
    "stdout": "partial output..."
}
```

---

## Error Responses

| HTTP Code | When                                |
| --------- | ----------------------------------- |
| `400`     | Invalid UUID in `/status/{id}`      |
| `404`     | Submission ID not found in Redis    |
| `408`     | Request timed out (5s server limit) |
| `500`     | Redis / internal error              |
| `503`     | Redis pool exhausted                |

---

## Supported Languages

| Value    | Language   | Filename      | Compiler / Runtime |
| -------- | ---------- | ------------- | ------------------ |
| `"cpp"`  | C++        | `main.cpp`    | `g++ -O2`          |
| `"rust"` | Rust       | `main.rs`     | `rustc -O`         |
| `"java"` | Java       | `Main.java`   | `javac` → `java`   |
| `"py"`   | Python     | `solution.py` | `python3`          |
| `"js"`   | JavaScript | `solution.js` | `bun`              |
| `"ts"`   | TypeScript | `solution.ts` | `bun`              |

---

## 6. Main App Database Schema

The following tables should be created in the Main App's database to manage the problem payload construction and to store the final submission results from the `/test` endpoint.

### Table 1: `problems`

This table stores the core execution constraints and test cases for a specific problem. The Main App queries this to build the `testcases`, `timelimit`, and `memorylimit` fields for the IronJudge JSON payload.

| Column Name    | Data Type | Constraints   | Description                                      |
| -------------- | --------- | ------------- | ------------------------------------------------ |
| `problem_id`   | UUID/Int  | Primary Key   | Unique identifier for the coding problem.        |
| `test_cases`   | JSON      | Not Null      | Stores the array of inputs and expected outputs. |
| `memory_limit` | Int       | Default: 256  | Maximum memory allowed in MB.                    |
| `time_limit`   | Int       | Default: 2000 | Maximum execution time allowed in ms.            |

### Table 2: `problem_languages`

This table stores the 6 different language entries for a single `problem_id`. It handles the separation between what the user sees and what is actually sent to IronJudge.

| Column Name   | Data Type | Constraints | Description                                                                                                                                           |
| ------------- | --------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `id`          | UUID/Int  | Primary Key | Unique identifier for this specific language configuration.                                                                                           |
| `problem_id`  | UUID/Int  | Foreign Key | References `problems.problem_id`.                                                                                                                     |
| `language`    | String    | Enum        | One of: `cpp`, `java`, `rust`, `js`, `ts`, `py`.                                                                                                      |
| `boilerplate` | Text      | Not Null    | The starter code shown to the user in the UI (e.g., just the function signature).                                                                     |
| `hidden_code` | Text      | Not Null    | The full wrapper code. For example, this is where the complete Python wrapper would be stored, while the `boilerplate` only shows the inner function. |

_How it works:_ When a user submits their code, the Main App fetches the `hidden_code` for that language, injects the user's submitted logic into it, and sends the combined string as the `code` field to IronJudge.

### Table 3: `test_submissions`

This table strictly logs the results of submissions sent to the `/test` endpoint. The `/run` endpoint is treated as a dry-run and is not recorded here.

| Column Name     | Data Type | Constraints | Description                                                                         |
| --------------- | --------- | ----------- | ----------------------------------------------------------------------------------- |
| `submission_id` | UUID      | Primary Key | The UUID returned by IronJudge.                                                     |
| `problem_id`    | UUID/Int  | Foreign Key | References `problems.problem_id`.                                                   |
| `user_id`       | UUID/Int  | Foreign Key | The ID of the user who made the submission.                                         |
| `status`        | String    | Not Null    | E.g., `pending`, `completed`, `error`. Updates via polling `/status`.               |
| `result_msg`    | String    | Nullable    | The `message` from IronJudge (e.g., `success`, `testcasefailed`, `run_time_error`). |
| `tests_passed`  | Int       | Default: 0  | Matches the `ttpassed` field from the IronJudge response.                           |

## Typical Flow

```
Client                          IronJudge
  |                                 |
  |--- POST /run  (or /test) ------>|
  |<-- 200 { submissionid }  -------|
  |                                 |
  |--- GET /status/{id}  --------->|
  |<-- 200 { status: processing } --|
  |                                 |
  |  (poll again after ~500ms)      |
  |--- GET /status/{id}  --------->|
  |<-- 200 { status: completed } ---|
```

---

## Limits

| Constraint       | Value |
| ---------------- | ----- |
| Max request body | 1 MB  |
| Request timeout  | 5 s   |
| Result TTL       | 600 s |
