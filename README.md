# Chassis (AI Garage)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021%20Edition-orange.svg)](https://www.rust-lang.org/)
[![Deployment Readiness](https://img.shields.io/badge/Deployment%20Readiness-All%20Gates%20Green-brightgreen.svg)](#deployment-readiness-gateway)
[![Security Audit](https://img.shields.io/badge/Vulnerabilities-0%20CVEs%20(RustSec)-brightgreen.svg)](#security--deployment-readiness)

**Chassis** is a sovereign, zero-trust **AI Plugin Microkernel** written in Rust.

Monolithic AI agent frameworks bundle assumptions, unsafe default host permissions, and sprawling dependency graphs. Chassis takes the opposite approach: **everything is an isolated plugin**, and the microkernel enforces **zero ambient authority**, mathematical capability attenuation, synchronous cryptographic event logging, and surgical process tree sanitization.

---

## Key Architectural Invariants

* 🛡️ **Zero Ambient Authority**: Plugins execute in isolated OS processes with scrubbed environments (`cmd.env_clear()`). No filesystem paths, sockets, or environment variables are accessible unless explicitly declared in a manifest, negotiated during handshake, and authorized by the host Capability Broker.
* ⚡ **Stdio JSON-RPC 2.0 NDJSON**: Standard newline-delimited JSON-RPC 2.0 wire protocol over `stdin`/`stdout`. Supports high-concurrency out-of-order multiplexing and instant sub-millisecond failover on process crash via pipe EOF draining.
* 📜 **Cryptographic Write-Ahead Log (WAL)**: Synchronous append-only ledger featuring a SHA-256 forward-hash chain:
  $$\text{Hash}_i = \text{SHA256}(\text{Hash}_{i-1} \parallel \text{Seq}_i \parallel \text{Timestamp}_i \parallel \text{EventType}_i \parallel \text{PayloadCanonicalBytes}_i)$$
  Every event is hashed over its exact canonical byte stream on disk, mathematically guaranteeing that a 1-bit mutation immediately invalidates replay audit verification.
* 📦 **Content-Addressed Blob Storage (`BlobStore`)**: Automatic offloading for payloads $> 256\text{ KB}$ using SHA-256 Content-Addressed Storage to preserve IPC pipe performance.
* 🔐 **Authenticated Secrets Vault (`EncryptedVault`)**: AES-256-GCM symmetric authenticated encryption with PBKDF2-HMAC-SHA256 (100,000 iterations) via `ring`. Secrets are stored with POSIX `0600` permissions and injected selectively in memory (`vault:<KEY>`) into authorized child processes only.
* 🤖 **Hierarchical Sub-Agent Delegation (`SubAgentManager`)**: Enforces the mathematical lease attenuation invariant:
  $$\text{Capabilities}(\text{Child}) \subseteq \text{Capabilities}(\text{Parent})$$
  Privilege escalation attempts are intercepted and rejected before execution. Sub-agents run under isolated sub-sessions with independent nested WAL files.
* 🧹 **Deep Process Tree Sanitization & Linux Subreaper**:
  - `PR_SET_CHILD_SUBREAPER` adoption prevents grandchildren or detached processes from escaping to PID 1.
  - Recursive `/proc/[0-9]*/stat` parent-process-ID tree parsing (`collect_descendants`) discovers all descendant processes.
  - `kill_process_tree` issues process group kill (`killpg`), targeted `SIGKILL` to all descendants, and non-blocking zombie reaping (`waitpid WNOHANG`).
* 🔌 **Interchangeable Local & Cloud Model Adapters**: Native integration with local OpenAI-compatible inference servers (`llama-server`, Ollama, vLLM) with tight 500ms connection timeouts and hermetic mock fallback simulation.
* 🧠 **Native Vector Search & Conversation Storage (`chassis-storage-sqlite`)**: Sovereign plugin engine powered by **libSQL (Turso)**. Features native Approximate Nearest Neighbor (ANN) cosine similarity search (`libsql_vector_idx`, `vector_top_k`) for message embeddings and semantic recall without external vector DB daemons.

---

## Workspace Architecture

```
ai-garage/
├── crates/
│   ├── chassis-protocol/         # Universal capability envelope, JSON-RPC 2.0 NDJSON types & error codes
│   ├── chassis-core/             # Microkernel runtime, capability router, WAL, vault, subagent, & supervisor
│   ├── chassis-cli/              # Sovereign command-line host binary (`chassis`)
│   └── plugins/
│       ├── chassis-model-local/  # Local model adapter (OpenAI-compatible / llama-server)
│       ├── chassis-tools-filesystem/ # Sandboxed workspace filesystem provider
│       └── chassis-storage-sqlite/   # Sovereign libSQL conversation & vector memory plugin
├── docs/
│   ├── DESIGN_LOG.md             # In-depth architectural specifications and state machines
│   └── DECISION_LOG.md           # Formal Architecture Decision Records (ADR-001 through ADR-031)
├── Makefile                      # Automated deployment readiness, testing, linting, & security scanning
└── Cargo.toml                    # Virtual workspace manifest
```

---

## Quickstart

### Prerequisites

* **Rust**: 1.80+ (stable toolchain)
* **Linux**: Recommended for native `PR_SET_CHILD_SUBREAPER` and `/proc` process isolation (macOS/Unix supported for development)
* **Optional Local Model**: Any OpenAI-compatible server (e.g., `llama-server` on port 8080)

### 1. Build the Workspace

```bash
# Debug build
make build

# Optimized release build
make build-release
```

### 2. Run the Deployment Readiness Gateway

Verify build correctness, complete test suite, strict clippy linter, and dependency security audit:

```bash
make ready
```

---

## CLI Usage

The sovereign CLI (`chassis`) provides workspace management, secret storage, microkernel execution, and audit replay.

```bash
# Run via cargo or target binary
alias chassis="cargo run -p chassis-cli --"
```

### Initialize a Sovereign Workspace
Creates `.chassis/` structure, default-deny `security_policy.toml`, and lockfiles:
```bash
chassis init /path/to/workspace
```

### Manage Encrypted Secrets
Store credentials in the AES-256-GCM vault with PBKDF2 derivation:
```bash
# Store a credential
chassis secret set OPENAI_API_KEY "sk-..."

# List stored keys (prints SHA-256 digests; plaintext is never exposed)
chassis secret list

# Delete a credential
chassis secret delete OPENAI_API_KEY
```

### Boot the Microkernel
Unlocks vault in memory, starts child plugin processes over stdio NDJSON, and enforces security firewall:
```bash
# Interactive mode
chassis run --workspace /path/to/workspace

# Non-interactive mode (fails fast on policy denial)
chassis run --workspace /path/to/workspace --non-interactive
```

### Replay & Verify Audit Ledger
Replays a recorded session WAL and mathematically verifies the cryptographic forward-hash chain:
```bash
chassis replay <session_id_or_wal_path> --workspace /path/to/workspace
```

---

## Security & Deployment Readiness

Chassis enforces continuous automated security verification as part of its development lifecycle.

Run the 4-gate deployment readiness check:
```bash
make ready
```

1. **Build Gate**: Checks compilation across all workspace crates and targets.
2. **Test Gate**: Runs 38 unit, integration, and stress tests.
3. **Lint Gate**: Strict Clippy analysis (`-D warnings`).
4. **Security Audit Gate**: Scans all 220 crate dependencies against the [RustSec Advisory Database](https://rustsec.org/) (Snyk equivalent) for known CVEs.

### Stress & Resilience Test Suite

Chassis includes a dedicated failure-injection and concurrency test suite:

```bash
make test-stress
```

* **STRESS-1 (Stdio Multiplexing)**: 1,000 asynchronous concurrent requests over OS stdio pipes with zero deadlocks or dropped requests.
* **STRESS-2 (Large Blob Spillover)**: 50 concurrent 512KB–2MB payloads stored and retrieved via `BlobStore`.
* **STRESS-3 (Sub-Agent Storm)**: 50 concurrent nested sub-agents and isolated sub-WAL session ledgers.
* **STRESS-4 (Abrupt Process Kill)**: Direct `SIGKILL` delivery to child process groups mid-request with instant EOF failover (< 100ms).
* **STRESS-5 (High-Throughput WAL)**: 1,000 sequential events committed with synchronous `fsync` and 100% hash chain validation.
* **STRESS-6 (Grandchild Reaping)**: Spawns detached grandchild background processes (`sleep 60 &`) and verifies subreaper adoption and complete elimination with **0 zombie processes** left in `/proc`.

---

## Architecture Documentation

For complete design specifications and architectural decisions, explore:
* 📘 [docs/DESIGN_LOG.md](docs/DESIGN_LOG.md): Full state machines, protocol lifecycle, blob threshold mechanics, and process sanitization.
* 📑 [docs/DECISION_LOG.md](docs/DECISION_LOG.md): 31 Architecture Decision Records (ADRs) documenting design rationale and trade-offs.

---

## License

This project is licensed under the **MIT License**. See the [LICENSE](LICENSE) file for details.
