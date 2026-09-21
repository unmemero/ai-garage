use crate::attenuation::EffectiveLease;
use crate::error::CoreError;
use crate::manifest::PluginManifest;
use chassis_protocol::jsonrpc::{Id, Message, Notification, Request, Response};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

/// A supervised plugin process with multiplexed asynchronous stdio transport
pub struct ProcessHandle {
    pub plugin_id: String,
    child: Child,
    stdin_tx: mpsc::Sender<Message>,
    pending_requests: Arc<Mutex<HashMap<Id, oneshot::Sender<Response>>>>,
    notification_tx: broadcast::Sender<Notification>,
    next_request_id: AtomicI64,
    pid: Option<u32>,
}

impl ProcessHandle {
    /// Spawn an isolated plugin child process with a scrubbed environment and process group isolation
    pub fn spawn(
        manifest: &PluginManifest,
        lease: &EffectiveLease,
        workspace_root: &Path,
        plugin_dir: &Path,
    ) -> Result<Self, CoreError> {
        let exec_path = if manifest.entrypoint.executable.is_absolute() {
            manifest.entrypoint.executable.clone()
        } else {
            let direct = plugin_dir.join(&manifest.entrypoint.executable);
            if direct.exists() {
                direct
            } else if let Some(file_name) = manifest.entrypoint.executable.file_name() {
                let target_debug = workspace_root.join("target/debug").join(file_name);
                let target_release = workspace_root.join("target/release").join(file_name);
                if target_debug.exists() {
                    target_debug
                } else if target_release.exists() {
                    target_release
                } else {
                    direct
                }
            } else {
                direct
            }
        };

        if !exec_path.exists() {
            return Err(CoreError::PolicyViolation(format!(
                "Plugin executable for '{}' not found. Checked: bundle path '{}' and dev fallback target/debug.",
                manifest.plugin.id,
                plugin_dir.join(&manifest.entrypoint.executable).display()
            )));
        }

        let mut cmd = Command::new(&exec_path);
        cmd.args(&manifest.entrypoint.args);
        cmd.current_dir(workspace_root);

        // 1. Zero Ambient Authority: Strip host environment variables completely
        cmd.env_clear();

        // 2. Pass safe minimal PATH so system dynamic linkers / runtime interpreters work
        if let Ok(path_var) = std::env::var("PATH") {
            cmd.env("PATH", path_var);
        }

        // 3. Inject declared passthroughs
        for var_name in &manifest.entrypoint.env_passthrough {
            if let Ok(val) = std::env::var(var_name) {
                cmd.env(var_name, val);
            }
        }

        // 4. Inject secrets authorized by the EffectiveLease
        for (secret_name, secret_val) in &lease.secrets {
            cmd.env(secret_name, secret_val);
        }

        // 5. Stdio Isolation: Piped stdin, stdout, and stderr
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // 6. Process Group (PGID) Isolation on Unix
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::PolicyViolation(format!(
                "Failed to spawn plugin '{}' at '{}': {}",
                manifest.plugin.id,
                exec_path.display(),
                e
            ))
        })?;

        let pid = child.id();
        let child_stdin = child.stdin.take().ok_or_else(|| {
            CoreError::PolicyViolation("Failed to capture child stdin pipe".into())
        })?;
        let child_stdout = child.stdout.take().ok_or_else(|| {
            CoreError::PolicyViolation("Failed to capture child stdout pipe".into())
        })?;
        let child_stderr = child.stderr.take().ok_or_else(|| {
            CoreError::PolicyViolation("Failed to capture child stderr pipe".into())
        })?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Message>(128);
        let pending_requests = Arc::new(Mutex::new(HashMap::<Id, oneshot::Sender<Response>>::new()));
        let (notification_tx, _) = broadcast::channel::<Notification>(256);

        // Stdin Writer Task: Serializes and flushes NDJSON messages line by line
        tokio::spawn(async move {
            let mut writer = BufWriter::new(child_stdin);
            while let Some(msg) = stdin_rx.recv().await {
                if let Ok(ndjson) = msg.to_ndjson() {
                    if writer.write_all(ndjson.as_bytes()).await.is_err() {
                        break;
                    }
                    if writer.flush().await.is_err() {
                        break;
                    }
                }
            }
        });

        // Stdout Reader Task: Multiplexed line reader routing responses and notifications
        let pending_clone = Arc::clone(&pending_requests);
        let notif_tx_clone = notification_tx.clone();
        let plugin_id_str = manifest.plugin.id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(child_stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match Message::from_ndjson(trimmed) {
                    Ok(Message::Response(res)) => {
                        let mut pending = pending_clone.lock().await;
                        if let Some(sender) = pending.remove(&res.id) {
                            let _ = sender.send(res);
                        } else {
                            tracing::warn!(
                                "[{}] Received response for unknown request ID: {:?}",
                                plugin_id_str,
                                res.id
                            );
                        }
                    }
                    Ok(Message::Notification(notif)) => {
                        let _ = notif_tx_clone.send(notif);
                    }
                    Ok(Message::Request(_req)) => {
                        tracing::debug!(
                            "[{}] Received reverse request from child process",
                            plugin_id_str
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[{}] Malformed JSON-RPC line received: {} (error: {})",
                            plugin_id_str,
                            trimmed,
                            e
                        );
                    }
                }
            }

            // Pipe reached EOF (process crashed, killed, or exited).
            // DRAIN all pending requests immediately so callers fail in sub-millisecond time!
            let mut pending = pending_clone.lock().await;
            for (id, sender) in pending.drain() {
                let err_resp = Response::error(
                    id,
                    chassis_protocol::RpcError::plugin_unavailable(format!(
                        "Plugin '{}' process terminated unexpectedly (stdio EOF)",
                        plugin_id_str
                    )),
                );
                let _ = sender.send(err_resp);
            }
        });

        // Stderr Reader Task: Logs child diagnostics
        let err_plugin_id = manifest.plugin.id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(child_stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::info!(target: "plugin::stderr", "[{}] {}", err_plugin_id, line);
            }
        });

        Ok(Self {
            plugin_id: manifest.plugin.id.clone(),
            child,
            stdin_tx,
            pending_requests,
            notification_tx,
            next_request_id: AtomicI64::new(1),
            pid,
        })
    }

    /// Send a multiplexed JSON-RPC request and await its specific response asynchronously
    pub async fn send_request(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<Response, CoreError> {
        let req_id = Id::Number(self.next_request_id.fetch_add(1, Ordering::SeqCst));
        let request = Request::new(req_id.clone(), method, params);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(req_id.clone(), tx);
        }

        if self
            .stdin_tx
            .send(Message::Request(request))
            .await
            .is_err()
        {
            let mut pending = self.pending_requests.lock().await;
            pending.remove(&req_id);
            return Err(CoreError::PolicyViolation(format!(
                "Failed to send request to plugin '{}' (stdin closed)",
                self.plugin_id
            )));
        }

        rx.await.map_err(|_| {
            CoreError::PolicyViolation(format!(
                "Plugin '{}' process exited or dropped channel before responding",
                self.plugin_id
            ))
        })
    }

    /// Send a one-way JSON-RPC notification
    pub async fn send_notification(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<(), CoreError> {
        let notif = Notification::new(method, params);
        self.stdin_tx
            .send(Message::Notification(notif))
            .await
            .map_err(|_| {
                CoreError::PolicyViolation(format!(
                    "Failed to send notification to plugin '{}' (stdin closed)",
                    self.plugin_id
                ))
            })
    }

    /// Subscribe to incoming notifications emitted by this plugin (e.g. streaming chunks)
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Notification> {
        self.notification_tx.subscribe()
    }

    /// Perform a clean shutdown with a graceful timeout, falling back to process group kill
    pub async fn shutdown(&mut self, timeout: std::time::Duration) -> Result<(), CoreError> {
        let shutdown_params = serde_json::json!({
            "grace_timeout_ms": timeout.as_millis(),
            "reason": "kernel_shutdown"
        });

        // 1. Attempt graceful shutdown request
        let graceful_attempt = tokio::time::timeout(
            timeout,
            self.send_request("kernel/shutdown", Some(shutdown_params)),
        )
        .await;

        if graceful_attempt.is_ok() {
            // Wait up to 500ms for process to exit cleanly
            if tokio::time::timeout(std::time::Duration::from_millis(500), self.child.wait())
                .await
                .is_ok()
            {
                return Ok(());
            }
        }

        // 2. Forceful teardown: Kill entire process group on Unix
        self.force_kill();
        let _ = self.child.wait().await;
        Ok(())
    }

    /// Forcefully terminate the process and its entire descendant process tree immediately
    pub fn force_kill(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            kill_process_tree(pid);
        }

        let _ = self.child.start_kill();
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.force_kill();
    }
}

/// Recursively discover all descendant PIDs of a process on Linux by traversing /proc
#[cfg(target_os = "linux")]
pub fn collect_descendants(root_pid: u32) -> Vec<u32> {
    let mut ppid_map: Vec<(u32, u32)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string() {
                if let Ok(pid) = file_name.parse::<u32>() {
                    let stat_path = entry.path().join("stat");
                    if let Ok(content) = std::fs::read_to_string(stat_path) {
                        if let Some(rparen) = content.rfind(')') {
                            let rest = content[rparen + 1..].trim_start();
                            let parts: Vec<&str> = rest.split_whitespace().collect();
                            if parts.len() >= 2 {
                                if let Ok(ppid) = parts[1].parse::<u32>() {
                                    ppid_map.push((pid, ppid));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut descendants = Vec::new();
    let mut queue = vec![root_pid];
    while let Some(current) = queue.pop() {
        for &(pid, ppid) in &ppid_map {
            if ppid == current && !descendants.contains(&pid) {
                descendants.push(pid);
                queue.push(pid);
            }
        }
    }
    descendants
}

#[cfg(not(target_os = "linux"))]
pub fn collect_descendants(_root_pid: u32) -> Vec<u32> {
    Vec::new()
}

/// Surgical termination of a process, its process group, and all discovered descendants
#[cfg(unix)]
pub fn kill_process_tree(root_pid: u32) {
    let descendants = collect_descendants(root_pid);

    unsafe {
        // 1. Kill the process group first (catches everything in the same PGID)
        libc::kill(-(root_pid as i32), libc::SIGKILL);

        // 2. Kill all discovered descendants directly (catches anything that called setsid or setpgid)
        for pid in descendants {
            libc::kill(pid as i32, libc::SIGKILL);
        }

        // 3. Reap any child zombies
        let mut status = 0;
        while libc::waitpid(-1, &mut status, libc::WNOHANG) > 0 {}
    }
}

#[cfg(not(unix))]
pub fn kill_process_tree(_root_pid: u32) {}
