# Chassis Implementation Task Checklist

This checklist tracks the implementation progress of the **Chassis** AI Plugin Microkernel.
Update this file as tasks are completed to maintain continuity across sessions.

---

## Phase 1: Workspace Scaffolding & Protocol Core
- [x] **1.1** Initialize Cargo multi-crate workspace (`chassis-core`, `chassis-protocol`, `chassis-cli`).
- [x] **1.2** Implement JSON-RPC 2.0 NDJSON transport framing and core message types (`Request`, `Response`, `Notification`, `Error`).
- [x] **1.3** Implement sovereign error code registry (`PolicyViolation`, `CapabilityNotFound`, `PluginUnavailable`, `ExecutionTimeout`, `UserRejected`).
- [x] **1.4** Implement Universal Capability Invocation Envelope (`capability/invoke`, `capability/stream_chunk`, `capability/abort`).
- [x] **1.5** Write unit tests for protocol parsing, streaming serialization, and invalid message rejection.
- [x] **1.6** Document Phase 1 components in `docs/` and walkthrough.

---

## Phase 2: Manifests & Security Policy Engine
- [x] **2.1** Implement `plugin.toml` manifest parser with strict type validation.
- [x] **2.2** Implement `security_policy.toml` host policy parser (workspaces, networks, HITL rules).
- [x] **2.3** Implement `plugins.lock.toml` parser with SHA-256 bundle verification.
- [x] **2.4** Implement Capability Broker intersection and policy attenuation algorithm.
- [x] **2.5** Write unit tests for permission attenuation, symlink path escapes, and policy overrides.
- [x] **2.6** Document Phase 2 components.

---

## Phase 3: Append-Only WAL Event Ledger
- [x] **3.1** Implement Write-Ahead Log writer with synchronous `fsync` guarantees (`.wal.jsonl`).
- [x] **3.2** Implement cryptographic SHA-256 hash chaining engine across session events.
- [x] **3.3** Implement WAL reader, integrity validator, and event replay scanner.
- [x] **3.4** Implement session active link manager (`active_session.link`).
- [x] **3.5** Write unit tests for event sequencing, tamper detection, and crash recovery.
- [x] **3.6** Document Phase 3 components.

---

## Phase 4: Process Supervisor & LIFO Lifecycle Engine
- [x] **4.1** Implement clean-environment process launcher with stripped environment variables.
- [x] **4.2** Implement Process Group (`setpgid`) isolation and surgical teardown (`killpg`).
- [x] **4.3** Implement asynchronous, multiplexed stdio message reader/writer over OS pipes.
- [x] **4.4** Implement Handshake state machine (`kernel/handshake` $\leftrightarrow$ `plugin/announce` $\rightarrow$ `handshake_ack`).
- [x] **4.5** Implement LIFO Revertible Effect Stack with RAII drops for deterministic cleanup.
- [x] **4.6** Write unit tests for child process lifecycle, ping/pong liveness, and process recovery.
- [x] **4.7** Document Phase 4 components.

---

## Phase 5: Capability Broker & Router Integration
- [x] **5.1** Implement runtime message router mapping `capability/invoke` to target child processes.
- [x] **5.2** Wire up the Capability Broker firewall to intercept requests before dispatch.
- [x] **5.3** Implement Human-in-the-Loop (HITL) gatekeeper routing (`ui.request_permission`).
- [x] **5.4** Implement execution mode handling (`--interactive`, `--non-interactive=fail`).
- [x] **5.5** Implement Blob Spillover protocol for payloads exceeding 256 KB.
- [x] **5.6** Write integration tests verifying end-to-end broker security gating.
- [x] **5.7** Document Phase 5 components.

---

## Phase 6: Reference Plugins & Microkernel Host
- [x] **6.1** Implement Reference Model Plugin (`chassis.model.local` / `llama-server` adapter supporting OpenAI-compatible local endpoints with mock fallback).
- [x] **6.2** Implement Reference Filesystem Tool Plugin (`chassis.tools.filesystem`) in Rust (`file_read`, `file_write`, `list_dir`).
- [x] **6.3** Implement CLI runner (`chassis-cli`) to discover, initialize workspace, and boot the microkernel.
- [x] **6.4** Document Phase 6 components and plugin bundle layouts.

---

## Phase 7: End-to-End Integration & Validation
- [x] **7.1** Implement end-to-end integration test harness.
- [x] **7.2** Execute full lifecycle test: Boot $\rightarrow$ Discover $\rightarrow$ Handshake $\rightarrow$ Model Request $\rightarrow$ Tool Call $\rightarrow$ WAL Flush $\rightarrow$ Clean Teardown.
- [x] **7.3** Verify deployment readiness (`make ready`).
- [x] **7.4** Document Phase 7 results.

---

## Phase 8: Encrypted Secrets Vault & Sub-Agent Support
- [x] **8.1** Implement AES-256-GCM credential vault (`~/.chassis/keychains/secrets.enc`).
- [x] **8.2** Implement CLI secret management commands (`chassis secret set/list/delete`).
- [x] **8.3** Implement selective process memory secret injection.
- [x] **8.4** Implement sub-agent spawning (`agent.spawn_subagent`) with attenuated leases and nested WAL.
- [x] **8.5** Write unit and integration tests for secrets and sub-agent lifecycles.
- [x] **8.6** Final documentation update.
