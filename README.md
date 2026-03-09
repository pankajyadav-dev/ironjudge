<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85-orange?style=for-the-badge&logo=rust&logoColor=white" />
  <img src="https://img.shields.io/badge/redis-streams-dc382d?style=for-the-badge&logo=redis&logoColor=white" />
  <img src="https://img.shields.io/badge/docker-ready-2496ED?style=for-the-badge&logo=docker&logoColor=white" />
  <img src="https://img.shields.io/badge/license-MIT-green?style=for-the-badge" />
</p>

<h1 align="center">⚔️ IronJudge</h1>

<p align="center">
  <strong>A blazing-fast, sandboxed code execution engine built in Rust.</strong><br/>
  Execute untrusted code securely with Linux-native isolation — namespaces, cgroups, chroot & seccomp.
</p>

<p align="center">
  <a href="#-features">Features</a> •
  <a href="#-architecture">Architecture</a> •
  <a href="#-supported-languages">Languages</a> •
  <a href="#-quickstart">Quickstart</a> •
  <a href="#-api-reference">API</a> •
  <a href="#-configuration">Config</a> •
  <a href="#-project-structure">Structure</a>
</p>

---

## 🚀 Features

- **Secure Sandbox** — Full process isolation via Linux namespaces (`PID`, `NET`, `MNT`, `UTS`), cgroups v2 resource limits, `chroot` filesystem isolation, and seccomp syscall filtering.
- **Multi-Language** — First-class support for **C++**, **Rust**, **Java**, **Python**, **JavaScript**, and **TypeScript**.
- **Async Job Pipeline** — Submissions flow through Redis Streams for decoupled, scalable processing.
- **Two Execution Modes** — **Run** (execute & return outputs) and **Test** (compare against expected answers).
- **Resource Limits** — Configurable per-submission time limits (ms) and memory limits (MB).
- **Production-Ready HTTP API** — Built on [Axum](https://github.com/tokio-rs/axum) with CORS, request timeouts, body-size limits, graceful shutdown, and structured tracing.
- **Docker-Ready** — Multi-stage Dockerfiles for both the API server and execution engine.

---

## 🏗 Architecture

```
┌─────────────┐       ┌───────────────────┐       ┌──────────────────┐
│   Client    │──────▶│  http_ironjudge   │──────▶│   Redis Stream   │
│  (REST)     │◀──────│  (Axum HTTP API)  │       │ (submission_stream)│
└─────────────┘       └───────────────────┘       └────────┬─────────┘
                              │ GET /status                │
                              ▼                            ▼
                      ┌───────────────┐          ┌──────────────────┐
                      │  Redis Store  │◀─────────│   x_ironjudge    │
                      │  (results)    │          │ (Execution Engine)│
                      └───────────────┘          └────────┬─────────┘
                                                          │
                                                          ▼
                                                 ┌──────────────────┐
                                                 │  Sandbox Runner  │
                                                 │  ┌────────────┐  │
                                                 │  │ namespaces │  │
                                                 │  │ cgroups v2 │  │
                                                 │  │ chroot     │  │
                                                 │  │ seccomp    │  │
                                                 │  └────────────┘  │
                                                 └──────────────────┘
```

**Flow:**

1. Client sends code via `POST /run` or `POST /test`.
2. `http_ironjudge` validates the request, generates a submission UUID, publishes the task to a Redis Stream, and returns the UUID immediately.
3. `x_ironjudge` consumes tasks from the stream via a consumer group, spawns an isolated sandbox for each submission, and writes results back to Redis.
4. Client polls `GET /status/{id}` until the result is available.

---

## 🌐 Supported Languages

| Language   | Key    | Filename      | Toolchain        |
| ---------- | ------ | ------------- | ---------------- |
| C++        | `cpp`  | `main.cpp`    | `g++ -O2`        |
| Rust       | `rust` | `main.rs`     | `rustc -O`       |
| Java       | `java` | `Main.java`   | `javac` → `java` |
| Python     | `py`   | `solution.py` | `python3`        |
| JavaScript | `js`   | `solution.js` | `bun`            |
| TypeScript | `ts`   | `solution.ts` | `bun`            |

---

## ⚡ Quickstart

### Prerequisites

- **Rust** ≥ 1.85 (stable)
- **Redis** ≥ 7.0 with Streams support
- **Linux** (sandbox requires kernel namespaces & cgroups v2)
- **Docker** (optional, for containerized deployment)
- Language toolchains for the languages you want to execute (g++, rustc, javac, python3, bun)

### Local Development

```bash
# 1 — Clone the repository
git clone https://github.com/pankajyadav-dev/ironjudge.git
cd ironjudge

# 2 — Configure environment
cp .env.local .env           # edit .env with your Redis URL, etc.

# 3 — Start Redis (if not already running)
docker run -d --name redis -p 6379:6379 redis:7-alpine

# 4 — Build & run the HTTP API server
cargo run -p http_ironjudge

# 5 — In a separate terminal, run the execution engine
#     (requires root/sudo for namespace & cgroup operations)
sudo cargo run -p x_ironjudge
```

### Docker Deployment

```bash
# Build the HTTP API server
docker build -f apps/http_ironjudge/Dockerfile -t ironjudge-http .

# Build the Execution Engine (requires --privileged at runtime)
docker build -f apps/x_ironjudge/Dockerfile -t ironjudge-engine .

# Run
docker run -d \
--name httpjudge \
--network ironnetwork \
--restart always \
-p 3000:3000 \
-e HTTPURL="0.0.0.0:3000" \
-e REDISURL="redis://api_user:api_pass@redis:6379/1" \
-e STREAMNAME="submission_stream" \
-e REDIS_POOL_SIZE="10" \
ironjudge-http


docker run -d \
  --name=judge \
  --cap-add=SYS_ADMIN \
--network ironnetwork \
  --cgroupns=private \
  --security-opt seccomp=unconfined \
  --security-opt apparmor=unconfined \
  --tmpfs /dev/shm:rw,exec \
  --tmpfs /sys/fs/cgroup:rw \
-e REDISURL="redis://api_user:api_pass@redis:6379/1" \
  -e STREAMNAME="submission_stream" \
  -e GROUPNAME="x_engine" \
  -e CONSUMERNAME="ironjudge" \
  -e REDISPAYLOADLEN="1" \
  xironjudge-engine:latest
```

---

## 📡 API Reference

> Full details with request/response examples are in [`API_CONTRACT.md`](./API_CONTRACT.md).

### Endpoints

| Method | Path           | Description                    |
| ------ | -------------- | ------------------------------ |
| `GET`  | `/`            | Health check                   |
| `POST` | `/run`         | Submit code (run mode)         |
| `POST` | `/test`        | Submit code (test mode)        |
| `GET`  | `/status/{id}` | Poll submission result by UUID |

### Quick Example

```bash
# Submit a TypeScript solution
curl -s -X POST http://localhost:3000/run \
  -H "Content-Type: application/json" \
  -d '{
    "code": "import * as fs from '\''fs'\'';\nconst n = parseInt(fs.readFileSync(0, '\''utf-8'\'').trim());\nfs.writeSync(3, (n * 2) + '\''\\n'\'');",
    "language": "ts",
    "testcases": [
      { "id": 1, "input": "5\n", "output": "10\n" },
      { "id": 2, "input": "0\n", "output": "0\n" }
    ]
  }'

# Response → { "submissionid": "a1b2c3d4-..." }

# Poll for results
curl -s http://localhost:3000/status/a1b2c3d4-...
```

### Response Statuses

| Status       | Message              | Meaning                        |
| ------------ | -------------------- | ------------------------------ |
| `pending`    | `processing`         | Queued, not yet picked up      |
| `processing` | `processing`         | Worker is executing            |
| `completed`  | `success`            | All test cases executed/passed |
| `completed`  | `testcasefailed`     | A test case failed (test mode) |
| `completed`  | `compile_time_error` | Compilation failed             |
| `completed`  | `run_time_error`     | Runtime crash                  |
| `completed`  | `time_limit_error`   | Exceeded time limit            |
| `completed`  | `memory_limit_error` | Exceeded memory limit          |
| `error`      | `error`              | Internal server error          |

### System Limits

| Constraint       | Value |
| ---------------- | ----- |
| Max request body | 1 MB  |
| Request timeout  | 5 s   |
| Result TTL       | 600 s |

---

## ⚙ Configuration

All configuration is via environment variables (loaded from `.env` / `.env.local`):

### HTTP API Server (`http_ironjudge`)

| Variable          | Required | Default | Description                          |
| ----------------- | -------- | ------- | ------------------------------------ |
| `HTTPURL`         | ✅       | —       | Bind address (e.g. `localhost:3000`) |
| `REDISURL`        | ✅       | —       | Redis connection string              |
| `STREAMNAME`      | ✅       | —       | Redis Stream key for submissions     |
| `REDIS_POOL_SIZE` | ❌       | auto    | Connection pool size                 |

### Execution Engine (`x_ironjudge`)

| Variable          | Required | Default | Description                        |
| ----------------- | -------- | ------- | ---------------------------------- |
| `REDISURL`        | ✅       | —       | Redis connection string            |
| `STREAMNAME`      | ✅       | —       | Redis Stream key to consume from   |
| `GROUPNAME`       | ✅       | —       | Consumer group name                |
| `CONSUMERNAME`    | ✅       | —       | Consumer identity within the group |
| `REDISPAYLOADLEN` | ✅       | —       | Batch size per stream read         |

---

## 📁 Project Structure

```
ironjudge/
├── apps/
│   ├── http_ironjudge/       # Axum HTTP API server
│   │   ├── src/main.rs       #   Routes, middleware, graceful shutdown
│   │   └── Dockerfile
│   └── x_ironjudge/          # Execution engine (stream consumer)
│       ├── src/main.rs       #   Redis Stream consumer loop
│       └── Dockerfile
├── libs/
│   ├── http_lib/             # Route handlers & request validation
│   ├── redis_lib/            # Connection pooling, stream ops, result storage
│   ├── sandbox_lib/          # Core sandbox: namespaces, cgroups, chroot, seccomp
│   └── types_lib/            # Shared types (payloads, configs, errors)
├── Cargo.toml                # Workspace manifest
├── API_CONTRACT.md           # Full API documentation
└── .env                      # Environment template
```

---

## 🔒 Security Model

IronJudge employs **defense-in-depth** to execute untrusted code safely:

| Layer              | Mechanism                                                                  |
| ------------------ | -------------------------------------------------------------------------- |
| **PID Namespace**  | Isolated process tree — sandboxed code cannot see or signal host processes |
| **NET Namespace**  | No network access from within the sandbox                                  |
| **MNT Namespace**  | Private mount tree with `chroot` into a minimal filesystem                 |
| **Cgroups v2**     | Hard memory limits; CPU time tracked per submission                        |
| **Seccomp BPF**    | Syscall allowlist — blocks dangerous operations at the kernel level        |
| **Time Limits**    | Configurable wall-clock timeout per submission (default 2 s)               |
| **Temp Isolation** | Each submission gets a unique temp directory, cleaned up after execution   |

---

## 🛠 Tech Stack

| Component        | Technology                                                                                |
| ---------------- | ----------------------------------------------------------------------------------------- |
| Language         | [Rust](https://www.rust-lang.org/) 1.85                                                   |
| HTTP Framework   | [Axum](https://github.com/tokio-rs/axum) 0.8                                              |
| Async Runtime    | [Tokio](https://tokio.rs/) (full features)                                                |
| Message Broker   | [Redis Streams](https://redis.io/docs/data-types/streams/)                                |
| Redis Client     | [redis-rs](https://github.com/redis-rs/redis-rs) + deadpool                               |
| Serialization    | [serde](https://serde.rs/) + serde_json                                                   |
| Sandboxing       | Linux namespaces, cgroups v2, chroot, [seccompiler](https://crates.io/crates/seccompiler) |
| Observability    | [tracing](https://docs.rs/tracing) + tower-http trace layer                               |
| Containerization | Docker (multi-stage builds on Debian Bookworm)                                            |

---

<p align="center">
  Built with 🦀 Rust — fast, safe, fearless.
</p>
