# Architecture Decision Log (ADL)

This log records all foundational architectural and design decisions for the sovereign AI plugin runtime.

---

### [ADR-001] Architectural Paradigm: Zero-Trust Microkernel
* **Status**: Accepted
* **Context**: DeepSeek Harness (`dsh`) demonstrated the value of an "everything is a plugin" model, but monolithic frameworks bundle too many assumptions, security risks, and unvetted dependencies.
* **Decision**: Adopt a strict **Microkernel architecture**. The core contains no built-in AI models, prompt logic, UI, or execution tools. Its sole responsibilities are process supervision, capability brokering, message routing, and append-only event logging.
* **Consequences**:
  * *Pros*: Maximum modularity, complete swappability, isolated failure domains.
  * *Cons*: Requires formal protocols and state management between modules.

---

### [ADR-002] Core Implementation Language: Rust
* **Status**: Accepted
* **Context**: The runtime must be performant, memory-safe, deterministic, and free of untrusted runtime dependencies (e.g., node_modules supply chain risks).
* **Decision**: Implement the microkernel core in **Rust**.
* **Consequences**:
  * *Pros*: Zero-cost abstractions, compile-time memory safety without a garbage collector, fearless concurrency, strong RAII primitives for deterministic resource cleanup, and first-class sandboxing support.
  * *Cons*: Steeper development curve than dynamic scripting languages.

---

### [ADR-003] Inter-Plugin Protocol: JSON-RPC 2.0 over Standard I/O (stdio)
* **Status**: Accepted
* **Context**: Binary IPC protocols (protobuf, flatbuffers) can obscure payloads, making inspection difficult. We require absolute auditability and transparency.
* **Decision**: Use standard **JSON-RPC 2.0** transmitted over child process **stdio** (`stdin`/`stdout`).
* **Consequences**:
  * *Pros*: 100% human-readable and inspectable; language-agnostic (plugins can be written in Rust, Python, Go, C, or Shell); process isolation out of the box (zero shared memory).
  * *Cons*: Slight serialization overhead compared to raw shared-memory binary formats; streaming large binary artifacts (e.g. video files) requires chunking or sidecar file descriptors.

---

### [ADR-004] Security Foundation: Zero Ambient Authority & Capability Broker
* **Status**: Accepted
* **Context**: Plugins may be untrusted, developed by third parties, or run code from varying origins. Default OS permissions are too permissive.
* **Decision**: Enforce **Zero-Trust capability-based security**. Plugins start with zero permissions. All operations requiring external interaction (disk, network, execution) must be declared in a manifest, validated during handshake, and gated per-call by a central Capability Broker.
* **Consequences**:
  * *Pros*: Compromised or malicious plugins cannot access files, sockets, or environment variables without explicit permission.
  * *Cons*: Requires clear error handling when permissions are denied.

---

### [ADR-005] Extensibility: Open Capability & Hook Registry (No Hardcoded Plugin Enums)
* **Status**: Accepted
* **Context**: Future AI capabilities (sensory input, hardware debugging, robotics, custom memory engines) will emerge beyond standard LLM/Tool/UI categories.
* **Decision**: The kernel will not maintain a static `enum PluginType`. Instead, plugins register as **Capability Providers** (advertising strings like `model.generate`, `tools.execute`, `custom.telemetry`) and **Event Subscribers**.
* **Consequences**:
  * *Pros*: Infinite extensibility without core rewrites.
  * *Cons*: Dynamic routing requires strict schema validation at runtime.

---

### [ADR-006] Lifecycle Model: Deterministic RAII & LIFO Teardown Stack
* **Status**: Accepted
* **Context**: Hot-reloading or crashing plugins in Node.js frameworks often causes resource leaks (orphaned processes, hung sockets).
* **Decision**: Implement a **LIFO (Last-In-First-Out) Reversible Effect Stack** backed by Rust's RAII ownership model. Every stateful effect registered by a plugin is tracked and guaranteed to be unwound upon termination.
* **Consequences**:
  * *Pros*: Zero leaked processes, handles, or memory. Guaranteed clean state on reload.

---

### [ADR-007] Distribution & Packaging: Decoupled Installer
* **Status**: Accepted
* **Context**: Core engine logic should remain clean and independent of platform-specific packaging and bootstrap mechanics.
* **Decision**: Packaging, environment bootstrap, and installation workflows are decoupled into a dedicated installer repository.

---

### [ADR-008] Transport Framing: Newline-Delimited JSON-RPC (NDJSON)
* **Status**: Accepted
* **Context**: We evaluated Content-Length framed JSON-RPC (LSP-style) versus Newline-Delimited JSON (NDJSON over stdio).
* **Decision**: Adopt **Newline-Delimited JSON (NDJSON)**. Every message is a strictly valid JSON-RPC 2.0 object terminated by a single `\n` character.
* **Consequences**:
  * *Pros*: Maximum human readability and simplicity. Trivial to inspect with Unix tools (`cat`, `jq`, `grep`), test from bash scripts, and parse line-by-line using standard stream buffers.
  * *Cons*: Any embedded newlines inside strings must be escaped (`\n`), which is native to standard JSON encoders anyway.

---

### [ADR-009] Universal Capability Invocation Envelope
* **Status**: Accepted
* **Context**: As new plugin types are added, creating unique point-to-point RPC methods in the microkernel for every new domain leads to kernel bloat and frequent rewrites.
* **Decision**: Route all domain-specific interactions through a **Universal Capability Invocation Envelope** (`capability/invoke`, `capability/stream_chunk`, `capability/abort`). Standard domains (`model`, `tools`, `context`, `ui`) define standardized schema payloads within this envelope.
* **Consequences**:
  * *Pros*: The microkernel router remains completely generic and domain-agnostic. Security inspection, telemetry logging, and timeout enforcement are identical across all plugin types.

---

### [ADR-010] Declarative TOML Manifests & The Principle of Least Privilege
* **Status**: Accepted
* **Context**: Plugins must statically declare their identity, capabilities, and requested system permissions so the kernel can inspect them prior to execution.
* **Decision**: Adopt a standardized `plugin.toml` manifest format for all plugins. Plugins must explicitly declare all required network domains, filesystem paths, and child processes. Unstated permissions are treated as non-existent.
* **Consequences**:
  * *Pros*: Completely auditable before a plugin is ever launched; enables automated pre-flight security linting and human verification.
  * *Cons*: Plugin authors must explicitly maintain manifest declarations when introducing new dependencies or endpoints.

---

### [ADR-011] Host-Governed Policy Attenuation
* **Status**: Accepted
* **Context**: A plugin may request certain capabilities, but the host user or deployment environment must have absolute authority to attenuate (restrict) or deny those requests.
* **Decision**: Implement a host-level `security_policy.toml` that acts as the ultimate authority. The Capability Broker computes the **intersection** of the Plugin Manifest and the Host Security Policy. A plugin can never receive more permissions than its manifest requests, but the Host Policy can unilaterally restrict or deny any requested permission.
* **Consequences**:
  * *Pros*: True zero-trust operation; prevents rogue plugins from escalating privileges even if compromised; supports air-gapped or restricted CI/CD environments.

---

### [ADR-012] Two-Tier Directory Topology (Global User vs Local Workspace)
* **Status**: Accepted
* **Context**: The runtime must balance machine-wide plugin installations and user defaults with project-specific security rules, lockfiles, and session traces.
* **Decision**: Adopt a **Two-Tier Topology**:
  1. *Global User Root* (`~/.chassis/` following XDG Base Directory standards): Houses centrally installed plugin binaries, base security defaults, and global keychains.
  2. *Local Workspace Root* (`<workspace>/.chassis/`): Houses project-specific policy overrides, the active `plugins.lock.toml`, session WAL event journals, and temporary scratch space.
* **Consequences**:
  * *Pros*: Projects are fully reproducible and auditable in version control while avoiding duplicate plugin installations across the disk.

---

### [ADR-013] Immutable Plugin Bundling & Checksum Validation
* **Status**: Accepted
* **Context**: To prevent supply-chain poisoning, a plugin binary or script must not be modifiable in place by unauthorized processes once verified.
* **Decision**: Installed plugins are stored in immutable, versioned directory bundles (`<plugin_id>/<semver>/`) containing a mandatory `checksums.sha256` manifest. The microkernel verifies these hashes upon discovery before spawning any process.
* **Consequences**:
  * *Pros*: Detects binary tampering, corrupted downloads, or unauthorized modifications immediately at boot.

---

### [ADR-014] Project Naming & Brand: Chassis (The AI Garage Skeleton)
* **Status**: Accepted
* **Context**: In automotive engineering, the "chassis" is the structural frame upon which all major components (engine, bodywork, steering, electronics) are mounted.
* **Decision**: Adopt **Chassis** as the project identity. Global configurations use `~/.chassis/` and local workspace states use `<workspace>/.chassis/`.

---

### [ADR-015] Append-Only Hash-Chained WAL Event Ledger
* **Status**: Accepted
* **Context**: We need complete auditability, crash resilience, session replayability, and tamper evidence without introducing a heavy external database.
* **Decision**: Store all session events in an **Append-Only Write-Ahead Log (WAL)** formatted as Newline-Delimited JSON (`.wal.jsonl`). Each event is synchronously flushed to disk before actions execute and contains a cryptographic hash of the previous event (hash chain).
* **Consequences**:
  * *Pros*: 100% human-readable; resilient to sudden crashes/power loss; enables deterministic time-travel replay and branching; provides cryptographic proof of all agent actions.
  * *Cons*: Requires periodic log compaction or checkpointing for very long-running autonomous sessions.

---

### [ADR-016] Blob Spillover Pattern for Large Payload Stdio Transport
* **Status**: Accepted
* **Context**: Streaming multi-megabyte payloads (large images, PDFs, binary dumps) directly over stdio JSON-RPC causes high memory spikes and serialization overhead.
* **Decision**: Payloads $> 256\text{ KB}$ spill over to local scratch storage (`<workspace>/.chassis/scratch/blobs/<sha256>`). The JSON-RPC message passes a lightweight `$blob` reference instead of inline base64 bytes.
* **Consequences**:
  * *Pros*: Keeps the stdio JSON wire fast and responsive; zero heap explosion; payloads are cached and content-addressed by hash.

---

### [ADR-017] Process Group (PGID) Sandboxing & Zombie Reaping
* **Status**: Accepted
* **Context**: Shell tools spawning compilers or background servers can leave detached orphan child processes if the parent tool process exits or cancels prematurely.
* **Decision**: The kernel and all tool plugins must assign a unique **Process Group ID (`setpgid`)** to spawned commands. Cancellation signals are delivered to the entire process group (`killpg(-pgid, SIGKILL)`), guaranteeing zero leaked background zombies.
* **Consequences**:
  * *Pros*: Completely prevents orphan compiler, test runner, or server processes from lingering in the background.

---

### [ADR-018] Out-of-Order Multiplexed Asynchronous Wire
* **Status**: Accepted
* **Context**: Parallel actions (e.g. concurrent file reads or multiple tool calls) must not block each other over a single stdio stream.
* **Decision**: Stdio transport is strictly multiplexed. Requests carry unique `call_id` tokens, and responses may be emitted asynchronously out-of-order by plugins.
* **Consequences**:
  * *Pros*: Eliminates head-of-line blocking across the single-pipe interface.

---

### [ADR-019] Execution Modes & Headless Gatekeeping
* **Status**: Accepted
* **Context**: Human-in-the-Loop confirmation prompts will cause headless CI/CD pipelines or cron jobs to hang indefinitely.
* **Decision**: Introduce explicit runtime execution flags: `--interactive` (default, prompts UI for approval), `--non-interactive=fail` (blocks and errors on any action requiring approval), and `--non-interactive=auto-approve` (restricted strictly to sandboxed disposable environments).
* **Consequences**:
  * *Pros*: Prevents CI/CD deadlocks while maintaining zero-trust policy enforcement.

---

### [ADR-020] Chassis Local Encrypted Secrets Vault
* **Status**: Accepted
* **Context**: API keys and tokens must not be stored in cleartext files, committed to git, or exposed ambiently in `/proc/<pid>/environ`.
* **Decision**: Store all credentials in a local AES-256-GCM encrypted vault (`~/.chassis/keychains/secrets.enc`). The kernel unlocks the vault in memory at startup and securely binds secrets only to authorized plugin child processes.
* **Consequences**:
  * *Pros*: Hardware- or passphrase-grade encryption at rest; zero plain-text secrets in git; plugins cannot snoop on each other's credentials.

---

### [ADR-021] Hierarchical Sub-Agent Delegation with Attenuated Capability Leases
* **Status**: Accepted
* **Context**: Complex reasoning workflows require delegating focused tasks to child agents (e.g. a researcher or code reviewer sub-agent).
* **Decision**: Sub-agents are first-class primitives spawned by the microkernel with strictly **attenuated** capability leases. A sub-agent can only possess a subset of its parent's permissions and operates in its own isolated sub-session lifecycle.
* **Consequences**:
  * *Pros*: True multi-agent composability without security escalation; isolated failure domains per sub-task.

---

### [ADR-022] Automated Deployment Readiness & Dependency Vulnerability Scanning
* **Status**: Accepted
* **Context**: To maintain zero-trust integrity, dependencies must be continuously audited for CVE vulnerabilities, code quality anti-patterns, and test regressions before any stage completion.
* **Decision**: Integrate **RustSec (`cargo audit`)** (the Rust ecosystem standard equivalent to Snyk) alongside `cargo clippy` and `cargo test` in a unified `make ready` gateway. Every phase must pass this 4-step readiness check with zero CVEs, zero warnings, and zero test failures.
* **Consequences**:
  * *Pros*: Immediate detection of vulnerable dependencies, supply chain alerts, and code smells without third-party proprietary agents.

---

### [ADR-023] OpenAI-Compatible Local Model Adapter with Sovereign Mock Simulation
* **Status**: Accepted
* **Context**: The model provider must be completely decoupled from the kernel and interchangeable. The user runs local inference via `llama-server` (e.g. `Meta-Llama-3.1-8B-Instruct-Q8_0.gguf` on port 8080).
* **Decision**: Implement `chassis.model.local` connecting to `http://localhost:8080/v1/chat/completions` (or `LLAMA_ENDPOINT`) via HTTP while exposing `model.generate` over stdio JSON-RPC 2.0 NDJSON to Chassis. Provide a hermetic mock fallback simulation for CI environments where the local GPU server is not booted.
* **Consequences**:
  * *Pros*: Native compatibility with local GGUF models (`llama-server`, vLLM, Ollama) and cloud endpoints (OpenAI, Anthropic adapters); zero network dependencies for offline integration testing.

---

### [ADR-024] Sandboxed Workspace Filesystem Plugin & Host CLI Bootstrapper
* **Status**: Accepted
* **Context**: Agents require workspace file operations (`file_read`, `file_write`, `list_dir`), and the host needs a sovereign CLI to initialize workspaces, boot the microkernel, and replay audit logs.
* **Decision**: Implement `chassis.tools.filesystem` as a sandboxed Rust plugin offering `tools.execute` capabilities strictly confined by the Capability Router firewall. Implement `chassis-cli` supporting `init`, `run`, `replay`, and `status`.
* **Consequences**:
  * *Pros*: End-to-end operational microkernel with live process supervision, capability negotiation, and 100% cryptographic audit trail verification.

---

### [ADR-025] Authenticated Secrets Vault & In-Memory Selective Child Injection
* **Status**: Accepted
* **Context**: Secret credentials (API tokens, auth keys) must never be stored in plaintext or ambiently accessible to untrusted plugins or child processes.
* **Decision**: Implement `EncryptedVault` using AES-256-GCM authenticated encryption and PBKDF2-HMAC-SHA256 (100,000 iterations) with random salt and nonces via `ring`. The vault is stored with `0600` permissions. The microkernel unlocks the vault in memory at boot and injects only explicitly authorized secrets (`vault:<KEY>`) into a plugin's sanitized environment, leaving the rest of the host environment scrubbed.
* **Consequences**:
  * *Pros*: High-grade cryptographic protection at rest, tamper detection via AEAD authentication tags, zero ambient leakages, zero dependencies outside standard verified crates.

---

### [ADR-026] Sub-Agent Attenuation Invariant & Nested WAL Sessions
* **Status**: Accepted
* **Context**: Autonomous orchestrators delegating sub-tasks must not be able to escalate permissions beyond their own policy lease, and sub-task activity must not clutter the root session ledger.
* **Decision**: Enforce the mathematical attenuation invariant $\text{Capabilities}(\text{Child}) \subseteq \text{Capabilities}(\text{Parent})$ in `SubAgentManager`. Sub-agents receive isolated sub-session IDs (`<parent_id>.sub.<child_id>`) and independent nested WAL logs (`<sessions_dir>/<parent_id>.sub.<child_id>.wal.jsonl`). The parent WAL only records high-level delegation events (`SUBAGENT_SPAWNED` and `SUBAGENT_RETURNED`).
* **Consequences**:
  * *Pros*: Zero privilege escalation across nested agent hierarchies; isolated failure domains; independent auditability per sub-task.

---

### [ADR-027] Hermetic Multi-Binary End-to-End Testing with Anti-Tamper Simulation
* **Status**: Accepted
* **Context**: Unit tests alone cannot prove that independent compiled binaries communicate properly over OS stdio pipes or that CLI flags interact correctly with persistent disk storage.
* **Decision**: Implement an end-to-end integration test (`e2e_chassis_complete.rs`) executing the real compiled `chassis-cli` binary in a temporary sandbox workspace. The test exercises the full operational lifecycle (`init` $\rightarrow$ `secret set/list` $\rightarrow$ `run` with live `chassis.model.local` and `chassis.tools.filesystem` $\rightarrow$ sub-agent delegation $\rightarrow$ `replay`), and deliberately injects bit-flip mutations into the WAL file to verify tamper detection.
* **Consequences**:
  * *Pros*: Proves end-to-end sovereign reliability, zero plaintext secret leakage, and cryptographic integrity under production-like conditions.

---

### [ADR-028] High-Concurrency Multiplexing and Failure-Injection Stress Testing
* **Status**: Accepted
* **Context**: Microkernels must maintain pipe stability and clean state even under heavy asynchronous loads or abrupt process failures.
* **Decision**: Implement a comprehensive stress test suite (`stress_tests.rs`) covering:
  1. High-concurrency stdio multiplexing (1,000 asynchronous requests).
  2. Concurrent multi-megabyte blob spillover (50 concurrent 512KB-2MB payloads).
  3. Sub-agent spawning storms (50 concurrent nested agents and WALs).
  4. Abrupt child process termination (`SIGKILL`) mid-flight to prove clean timeout/error handling, LIFO unwinding, and zero zombie processes.
  5. High-throughput synchronous WAL append with forward-hash validation.
* **Consequences**:
---

### [ADR-029] Pipe EOF Instant-Failure Draining and Fast Network Timeouts
* **Status**: Accepted
* **Context**: When a plugin child process unexpectedly crashes or is killed (SIGSEGV/SIGKILL), callers with in-flight RPC requests previously had to wait out the entire invocation timeout (e.g. 30 seconds). Furthermore, when querying remote or external model endpoints that are blackholed or down, default HTTP connection timeouts can block execution indefinitely.
* **Decision**: 
  1. In `ProcessHandle::spawn`, monitor the stdout reader pipe. Upon detecting EOF (`Ok(0)` or error), immediately drain `pending_requests` and signal all in-flight caller oneshots with a `-32003 PluginUnavailable` error response, collapsing failover latency from tens of seconds to sub-millisecond.
  2. In `chassis-model-local`, enforce a tight connection timeout (`connect_timeout(500ms)`) on outbound HTTP requests, causing unreachable external endpoints to instantly fail over to sovereign simulation mode without blocking the pipeline.
* **Consequences**:
  * *Pros*: In-flight calls fail instantly upon process crash; resilient against network partition / dead IP blackholing.
  * *Cons*: Requires thread-safe sharing of pending request map between reader loop and RPC callers.

---

### [ADR-030] Linux Subreaper Adoption and Recursive Descendant Process Tree Reaping
* **Status**: Accepted
* **Context**: Untrusted or misbehaved plugins may fork child or grandchild processes, call `setsid()`, or detach from the process group. If the plugin is killed, these orphaned background processes can become zombies or linger indefinitely, consuming system resources.
* **Decision**: 
  1. Initialize `PR_SET_CHILD_SUBREAPER` via `libc::prctl` on Linux during microkernel boot. Any grandchild processes that become orphaned by double-forking or parent death are reparented directly to the Chassis host microkernel instead of PID 1 (`systemd`).
  2. Implement recursive `/proc/[0-9]*/stat` parent-process-ID (PPID) tree parsing (`collect_descendants`) to locate all descendant processes across all process groups.
  3. When terminating or dropping a plugin, invoke `kill_process_tree`, which issues `killpg` (PGID kill), direct `SIGKILL` to all discovered descendants, and non-blocking zombie reaping (`waitpid(-1, WNOHANG)`).
* **Consequences**:
  * *Pros*: Completely eliminates zombie and orphaned background processes; guaranteed 100% process tree clean-up.
  * *Cons*: Relies on Linux `/proc` filesystem and POSIX signaling; non-Unix fallback uses process handle termination.









