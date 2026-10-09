mod ui_server;

use chassis_core::{
    init_subreaper, BlobStore, CapabilityRouter, EncryptedVault, ExecutionMode, PluginLockfile,
    PluginManifest, PluginSupervisor, SecurityPolicy, WalReader, WalWriter,
};
use chassis_protocol::InvokeRequest;
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use ui_server::{PluginCapabilityInfo, PluginInfo, PluginPermissionsInfo, UiState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_subreaper();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "init" => {
            let workspace = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            cmd_init(&workspace)?;
        }
        "run" => {
            let mut workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut non_interactive = false;

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--workspace" | "-w" => {
                        if i + 1 < args.len() {
                            workspace = PathBuf::from(&args[i + 1]);
                            i += 1;
                        }
                    }
                    "--non-interactive" => {
                        non_interactive = true;
                    }
                    _ => {}
                }
                i += 1;
            }

            cmd_run(&workspace, non_interactive, None).await?;
        }
        "goal" => {
            if args.len() < 3 {
                eprintln!("Usage: chassis goal <GOAL_TEXT> [--workspace <DIR>] [--non-interactive]");
                std::process::exit(1);
            }
            let goal_text = args[2].clone();
            let mut workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut non_interactive = false;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--workspace" | "-w" => {
                        if i + 1 < args.len() {
                            workspace = PathBuf::from(&args[i + 1]);
                            i += 1;
                        }
                    }
                    "--non-interactive" => {
                        non_interactive = true;
                    }
                    _ => {}
                }
                i += 1;
            }

            cmd_run(&workspace, non_interactive, Some(goal_text)).await?;
        }
        "replay" => {
            if args.len() < 3 {
                eprintln!("Usage: chassis replay <session_id_or_wal_path> [--workspace <dir>]");
                std::process::exit(1);
            }
            let target = &args[2];
            let mut workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if let Some(pos) = args.iter().position(|a| a == "--workspace" || a == "-w") {
                if pos + 1 < args.len() {
                    workspace = PathBuf::from(&args[pos + 1]);
                }
            }
            cmd_replay(target, &workspace)?;
        }
        "status" => {
            let workspace = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            cmd_status(&workspace)?;
        }
        "ui" => {
            let mut workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut port: u16 = 3000;
            let mut host = "127.0.0.1".to_string();

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--workspace" | "-w" if i + 1 < args.len() => {
                        workspace = PathBuf::from(&args[i + 1]);
                        i += 1;
                    }
                    "--port" | "-p" if i + 1 < args.len() => {
                        port = args[i + 1].parse().unwrap_or(3000);
                        i += 1;
                    }
                    "--host" if i + 1 < args.len() => {
                        host = args[i + 1].clone();
                        i += 1;
                    }
                    _ => {}
                }
                i += 1;
            }

            cmd_ui(&workspace, &host, port).await?;
        }
        "secret" => {
            cmd_secret(&args[2..])?;
        }
        "--help" | "-h" | "help" => {
            print_usage();
        }
        cmd => {
            eprintln!("Unknown command: {cmd}\n");
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}

fn print_usage() {
    println!(
        r#"Chassis AI Microkernel v{}
Zero-trust, sovereign plugin microkernel where everything is a plugin.

USAGE:
    chassis <COMMAND> [OPTIONS]

COMMANDS:
    init [PATH]                   Initialize .chassis workspace structure
    run [OPTIONS]                 Boot microkernel and run agent loop
    ui [OPTIONS]                  Launch sovereign local web dashboard (http://127.0.0.1:3000)
    goal <GOAL> [OPTIONS]         Execute an autonomous goal using ReAct orchestrator
    replay <SESSION_OR_FILE>      Replay and verify cryptographically chained WAL session
    status [PATH]                 Check status of workspace and active sessions
    secret <SUBCOMMAND>           Manage AES-256-GCM encrypted credentials vault
    help                          Print this message

SECRET SUBCOMMANDS:
    set <KEY> [VALUE]             Set an encrypted secret in the vault
    list                          List secret keys and their hash digests
    delete <KEY>                  Remove a secret key from the vault

OPTIONS:
    -w, --workspace <PATH>        Specify workspace directory (default: current directory)
    -p, --port <PORT>             Port for web dashboard (default: 3000)
    --host <HOST>                 Host for web dashboard (default: 127.0.0.1)
    --non-interactive             Run without interactive prompts (fail on permission demand)
"#,
        env!("CARGO_PKG_VERSION")
    );
}

fn cmd_init(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Initializing Chassis sovereign workspace at: {}", workspace.display());

    let chassis_dir = workspace.join(".chassis");
    let sessions_dir = chassis_dir.join("sessions");
    let blobs_dir = chassis_dir.join("blobs");
    let plugins_dir = chassis_dir.join("plugins");
    let keychains_dir = chassis_dir.join("keychains");

    fs::create_dir_all(&sessions_dir)?;
    fs::create_dir_all(&blobs_dir)?;
    fs::create_dir_all(&plugins_dir)?;
    fs::create_dir_all(&keychains_dir)?;

    let policy_path = chassis_dir.join("security_policy.toml");
    if !policy_path.exists() {
        let default_policy = format!(
            r#"[policy]
version = "1.0.0"
default_action = "deny"
enforce_strict_workspaces = true

[workspace]
root = "{}"
allow_absolute_paths_outside_root = false
forbidden_patterns = ["**/.git/**", "**/.env*", "**/secrets/**"]

[network]
mode = "whitelist_only"
global_allowed_domains = ["localhost", "127.0.0.1", "0.0.0.0", "api.duckduckgo.com", "html.duckduckgo.com", "duckduckgo.com", "en.wikipedia.org", "wikipedia.org"]
blacklisted_domains = []

[human_in_the_loop]
require_confirmation_for = ["tools.execute:file_write", "tools.execute:shell_execute"]
critical_command_patterns = ["rm -rf *", "git push --force*"]
"#,
            workspace.display()
        );
        fs::write(&policy_path, default_policy)?;
        println!("  ✓ Created security policy: {}", policy_path.display());
    }

    let lockfile_path = chassis_dir.join("plugins.lock.toml");
    if !lockfile_path.exists() {
        let default_lockfile = r#"[lockfile]
version = "1.0.0"
generated_at = "2026-09-21T00:00:00Z"
"#;
        fs::write(&lockfile_path, default_lockfile)?;
        println!("  ✓ Created plugin lockfile: {}", lockfile_path.display());
    }

    println!("  ✓ Created sessions directory: {}", sessions_dir.display());
    println!("  ✓ Created blobs directory: {}", blobs_dir.display());
    println!("  ✓ Created plugins directory: {}", plugins_dir.display());
    println!("  ✓ Created keychains directory: {}", keychains_dir.display());
    println!("🚀 Workspace initialized successfully!");
    Ok(())
}

fn get_vault_path(workspace: &Path) -> PathBuf {
    let local = workspace.join(".chassis/keychains/secrets.enc");
    if local.exists() || workspace.join(".chassis").exists() {
        local
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".chassis/keychains/secrets.enc")
    } else {
        local
    }
}

fn get_vault_passphrase() -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(pass) = env::var("CHASSIS_VAULT_PASSWORD") {
        if !pass.is_empty() {
            return Ok(pass);
        }
    }
    // Default fallback master passphrase for development/testing if not specified in env
    Ok("chassis_default_sovereign_master_key".to_string())
}

fn cmd_secret(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("Usage: chassis secret <set|list|delete> [ARGS] [--workspace <DIR>]");
        std::process::exit(1);
    }

    let mut workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(pos) = args.iter().position(|a| a == "--workspace" || a == "-w") {
        if pos + 1 < args.len() {
            workspace = PathBuf::from(&args[pos + 1]);
        }
    }

    let vault_path = get_vault_path(&workspace);
    let passphrase = get_vault_passphrase()?;

    let mut vault = if vault_path.exists() {
        EncryptedVault::load_from_file(&vault_path, &passphrase)?
    } else {
        EncryptedVault::new()
    };

    match args[0].as_str() {
        "set" => {
            if args.len() < 2 {
                eprintln!("Usage: chassis secret set <KEY> [VALUE]");
                std::process::exit(1);
            }
            let key = &args[1];
            let value = if args.len() >= 3 && !args[2].starts_with('-') {
                args[2].clone()
            } else {
                eprint!("Enter secret value for '{}': ", key);
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                input.trim().to_string()
            };

            vault.set(key, value);
            vault.save_to_file(&vault_path, &passphrase)?;
            println!("🔒 Secret '{}' stored securely in encrypted vault.", key);
        }
        "list" => {
            let keys = vault.list_keys();
            println!("🔑 Stored secrets in {}: ({} keys)", vault_path.display(), keys.len());
            if keys.is_empty() {
                println!("  (No secrets stored yet. Use 'chassis secret set <KEY> <VALUE>' to add)");
            } else {
                println!("{:-<50}", "");
                println!("{:<25} | {:<20}", "KEY", "DIGEST");
                println!("{:-<50}", "");
                for k in keys {
                    let val = vault.get(&k).unwrap_or("");
                    let mut hasher = Sha256::new();
                    hasher.update(val.as_bytes());
                    let digest = format!("sha256:{:.8}...", format!("{:x}", hasher.finalize()));
                    println!("{:<25} | {:<20}", k, digest);
                }
                println!("{:-<50}", "");
            }
        }
        "delete" => {
            if args.len() < 2 {
                eprintln!("Usage: chassis secret delete <KEY>");
                std::process::exit(1);
            }
            let key = &args[1];
            if vault.delete(key).is_some() {
                vault.save_to_file(&vault_path, &passphrase)?;
                println!("✓ Secret '{}' deleted from encrypted vault.", key);
            } else {
                println!("Secret '{}' was not found in vault.", key);
            }
        }
        other => {
            eprintln!("Unknown secret subcommand: {other}");
            eprintln!("Available subcommands: set, list, delete");
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn cmd_run(
    workspace: &Path,
    non_interactive: bool,
    custom_goal: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("⚡ Booting Chassis sovereign microkernel...");
    let chassis_dir = workspace.join(".chassis");
    if !chassis_dir.exists() {
        eprintln!("Error: .chassis directory not found in {}. Run 'chassis init' first.", workspace.display());
        std::process::exit(1);
    }

    let policy_path = chassis_dir.join("security_policy.toml");
    let policy = if policy_path.exists() {
        let policy_str = fs::read_to_string(&policy_path)?;
        SecurityPolicy::from_toml(&policy_str)?
    } else {
        let mut p = SecurityPolicy::default();
        p.workspace.root = workspace.to_path_buf();
        p
    };

    // Attempt to unlock encrypted vault if present
    let vault_path = get_vault_path(workspace);
    let unlocked_vault = if vault_path.exists() {
        if let Ok(pass) = get_vault_passphrase() {
            match EncryptedVault::load_from_file(&vault_path, &pass) {
                Ok(v) => {
                    println!("🔓 Unlocked encrypted vault: {} ({} secrets loaded)", vault_path.display(), v.len());
                    Some(v)
                }
                Err(e) => {
                    eprintln!("⚠️ Failed to unlock encrypted vault: {e}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let session_id = format!("ses_{}", Utc::now().format("%Y%m%d_%H%M%S"));
    let sessions_dir = chassis_dir.join("sessions");
    let mut wal = WalWriter::init(&session_id, &sessions_dir)?;
    wal.append("SESSION_START", json!({ "workspace": workspace.display().to_string(), "mode": if non_interactive { "non-interactive" } else { "interactive" } }))?;
    println!("📝 WAL Session initialized: {session_id}");

    let blobs_dir = chassis_dir.join("blobs");
    let blob_store = BlobStore::new(&blobs_dir)?;
    let exec_mode = if non_interactive {
        ExecutionMode::NonInteractiveFail
    } else {
        ExecutionMode::Interactive
    };

    let router = Arc::new(CapabilityRouter::new(policy.clone(), workspace, blob_store, exec_mode, None));
    let reverse_channel = router.create_reverse_channel();

    // Dynamically locate plugin manifests across:
    // 1. In-tree development plugins: <workspace>/crates/plugins/*/plugin.toml
    // 2. Installed workspace plugins: <workspace>/.chassis/plugins/*/plugin.toml
    let mut plugins_to_boot = Vec::new();
    let search_roots = [
        workspace.join("crates/plugins"),
        chassis_dir.join("plugins"),
    ];

    for root in &search_roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let manifest_cand = entry.path().join("plugin.toml");
                if manifest_cand.is_file() && !plugins_to_boot.contains(&manifest_cand) {
                    plugins_to_boot.push(manifest_cand);
                }
            }
        }
    }

    println!("🔍 Discovered {} candidate plugin manifests", plugins_to_boot.len());

    let mut booted_count = 0;
    for manifest_path in &plugins_to_boot {
        let manifest_content = fs::read_to_string(manifest_path)?;
        let manifest = PluginManifest::from_toml(&manifest_content)?;
        let plugin_id = manifest.plugin.id.clone();
        let plugin_dir = manifest_path.parent().unwrap_or(workspace);

        println!("🔌 Booting plugin: {plugin_id} (v{})", manifest.plugin.version);
        match PluginSupervisor::launch_and_handshake_full(
            manifest,
            &policy,
            workspace,
            plugin_dir,
            &session_id,
            Duration::from_secs(5),
            unlocked_vault.as_ref(),
            Some(reverse_channel.clone()),
        )
        .await
        {
            Ok(supervisor) => {
                let caps = supervisor.manifest.capabilities_offered.clone();
                for cap in &caps {
                    println!("   Offer capability: {} (methods: {:?})", cap.id, cap.methods);
                }
                router.register_plugin(supervisor).await;
                wal.append(
                    "PLUGIN_BOOTED",
                    json!({
                        "plugin_id": plugin_id,
                        "capabilities": caps.iter().map(|c| &c.id).collect::<Vec<_>>()
                    }),
                )?;
                booted_count += 1;
            }
            Err(e) => {
                eprintln!("   ⚠️ Failed to launch plugin {plugin_id}: {e}");
            }
        }
    }

    println!("✅ Boot complete: {booted_count} plugins online and verified.");

    if let Some(goal) = custom_goal {
        println!("\n🧠 Dispatching ReAct Agent Goal: \"{}\"", goal);
        let orch_req = InvokeRequest::new(
            "req_goal_custom",
            "agent.orchestrate",
            "run_goal",
            json!({
                "goal": goal,
                "max_steps": 5,
                "conversation_id": format!("conv_{session_id}")
            }),
        );

        wal.append("CAPABILITY_DISPATCH_START", json!({ "target": "agent.orchestrate", "method": "run_goal" }))?;
        let orch_resp = router.dispatch("agent_core", orch_req).await?;
        wal.append("CAPABILITY_DISPATCH_RESULT", json!({ "response": orch_resp }))?;

        if let Some(err) = orch_resp.error {
            println!("   ❌ Agent execution error: {}", err.message);
        } else if let Some(res) = orch_resp.result {
            println!("\n🎯 Agent execution completed successfully!");
            if let Some(final_ans) = res.get("final_answer").and_then(|a| a.as_str()) {
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!("🏁 Final Answer:\n{}", final_ans);
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            }
            if let Some(steps) = res.get("history").and_then(|h| h.as_array()) {
                println!("\n📋 Trajectory steps ({}):", steps.len());
                for s in steps {
                    let step_num = s.get("step").and_then(|n| n.as_u64()).unwrap_or(0);
                    let thought = s.get("thought").and_then(|t| t.as_str()).unwrap_or("");
                    println!("  [Step {}] Thought: {}", step_num, thought);
                    if let Some(act) = s.get("action") {
                        let cap = act.get("capability").and_then(|c| c.as_str()).unwrap_or("");
                        let method = act.get("method").and_then(|m| m.as_str()).unwrap_or("");
                        println!("           Action: {}:{}", cap, method);
                    }
                    if let Some(obs) = s.get("observation") {
                        println!("           Observation: {}", obs);
                    }
                }
            }
        }
    } else {
        // Demonstration self-test runs
        println!("\n🤖 Executing self-test capability dispatch...");

        // 1. Model generation invocation
        let model_req = InvokeRequest::new(
            "req_model_001",
            "model.generate",
            "generate",
            json!({
                "messages": [
                    { "role": "system", "content": "You are Chassis AI sovereign kernel." },
                    { "role": "user", "content": "Hello sovereign agent" }
                ]
            }),
        );

        wal.append("CAPABILITY_DISPATCH_START", json!({ "target": "model.generate", "method": "generate" }))?;
        let model_resp = router.dispatch("agent_core", model_req).await?;
        wal.append("CAPABILITY_DISPATCH_RESULT", json!({ "response": model_resp }))?;
        if let Some(err) = model_resp.error {
            println!("   ❌ Model generation error: {}", err.message);
        } else {
            println!("   ✓ Model generation response received: {}", model_resp.result.unwrap_or_default());
        }

        // 2. Directory listing invocation
        let fs_req = InvokeRequest::new(
            "req_fs_001",
            "tools.execute",
            "list_dir",
            json!({ "path": "." }),
        );

        wal.append("CAPABILITY_DISPATCH_START", json!({ "target": "tools.execute", "method": "list_dir" }))?;
        let fs_resp = router.dispatch("agent_core", fs_req).await?;
        wal.append("CAPABILITY_DISPATCH_RESULT", json!({ "response": fs_resp }))?;
        if let Some(err) = fs_resp.error {
            println!("   ❌ Filesystem tools error: {}", err.message);
        } else {
            println!("   ✓ Filesystem tools response received: {}", fs_resp.result.unwrap_or_default());
        }

        // 3. Storage conversation invocation
        let storage_req = InvokeRequest::new(
            "req_storage_001",
            "storage.conversation",
            "conversation_create",
            json!({
                "id": format!("conv_{session_id}"),
                "title": "Sovereign E2E Self-Test Session",
                "model_id": "chassis.model.local",
                "chassis_session_id": session_id
            }),
        );

        wal.append("CAPABILITY_DISPATCH_START", json!({ "target": "storage.conversation", "method": "conversation_create" }))?;
        let storage_resp = router.dispatch("agent_core", storage_req).await?;
        wal.append("CAPABILITY_DISPATCH_RESULT", json!({ "response": storage_resp }))?;
        if let Some(err) = storage_resp.error {
            println!("   ⚠️ Storage conversation status: {}", err.message);
        } else {
            println!("   ✓ Storage conversation created: {}", storage_resp.result.unwrap_or_default());
        }

        // 4. Autonomous agent orchestrator invocation
        let orch_req = InvokeRequest::new(
            "req_orch_001",
            "agent.orchestrate",
            "run_goal",
            json!({
                "goal": "Inspect workspace files and report project status",
                "max_steps": 3,
                "conversation_id": format!("conv_{session_id}")
            }),
        );

        wal.append("CAPABILITY_DISPATCH_START", json!({ "target": "agent.orchestrate", "method": "run_goal" }))?;
        let orch_resp = router.dispatch("agent_core", orch_req).await?;
        wal.append("CAPABILITY_DISPATCH_RESULT", json!({ "response": orch_resp }))?;
        if let Some(err) = orch_resp.error {
            println!("   ⚠️ Agent orchestrator status: {}", err.message);
        } else {
            println!("   ✓ Agent orchestrator goal executed: {}", orch_resp.result.unwrap_or_default());
        }
    }

    wal.append("SESSION_END", json!({ "status": "clean_exit", "booted_plugins": booted_count }))?;
    println!("\n🏁 Microkernel session finished successfully. WAL event chain fully flushed.");
    Ok(())
}

fn cmd_replay(target: &str, workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let wal_path = if target.ends_with(".wal.jsonl") {
        PathBuf::from(target)
    } else {
        workspace.join(".chassis/sessions").join(format!("{target}.wal.jsonl"))
    };

    if !wal_path.exists() {
        eprintln!("Error: WAL file not found at: {}", wal_path.display());
        std::process::exit(1);
    }

    println!("📜 Replaying and verifying WAL ledger: {}", wal_path.display());
    let reader = WalReader::open(&wal_path)?;
    let events = reader.validate_integrity()?;

    println!("🔒 Cryptographic verification: PASSED ({} events)", events.len());
    println!("{:-<80}", "");
    println!("{:<5} | {:<20} | {:<25} | {:<20}", "SEQ", "TIMESTAMP", "EVENT TYPE", "HASH");
    println!("{:-<80}", "");

    for ev in &events {
        let short_hash = if ev.hash.len() > 16 {
            &ev.hash[..16]
        } else {
            &ev.hash
        };
        println!(
            "{:<5} | {:<20} | {:<25} | {:<20}...",
            ev.seq, ev.timestamp_utc, ev.event_type, short_hash
        );
    }
    println!("{:-<80}", "");
    println!("Integrity: 100% untampered SHA-256 forward-hash chain.");
    Ok(())
}

fn cmd_status(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let chassis_dir = workspace.join(".chassis");
    println!("🔎 Inspecting Chassis workspace: {}", workspace.display());
    if !chassis_dir.exists() {
        println!("Status: NOT INITIALIZED. (Run 'chassis init' to initialize)");
        return Ok(());
    }

    let policy_path = chassis_dir.join("security_policy.toml");
    let lockfile_path = chassis_dir.join("plugins.lock.toml");
    let sessions_dir = chassis_dir.join("sessions");
    let blobs_dir = chassis_dir.join("blobs");
    let keychains_dir = chassis_dir.join("keychains");

    println!("  Policy file: {}", if policy_path.exists() { "Present" } else { "Missing" });
    if policy_path.exists() {
        let policy_str = fs::read_to_string(&policy_path)?;
        if let Ok(p) = SecurityPolicy::from_toml(&policy_str) {
            println!("    Default action: {}", p.policy.default_action);
            println!("    Network mode: {:?}", p.network.mode);
            println!("    Enforce strict workspaces: {}", p.policy.enforce_strict_workspaces);
        }
    }

    println!("  Lockfile: {}", if lockfile_path.exists() { "Present" } else { "Missing" });
    if lockfile_path.exists() {
        let lock_str = fs::read_to_string(&lockfile_path)?;
        if let Ok(l) = PluginLockfile::from_toml(&lock_str) {
            println!("    Locked plugins: {}", l.plugins.len());
        }
    }

    let vault_path = keychains_dir.join("secrets.enc");
    println!("  Encrypted Vault: {}", if vault_path.exists() { "Present (secrets.enc)" } else { "Not created" });

    let mut session_count = 0;
    if let Ok(entries) = fs::read_dir(&sessions_dir) {
        session_count = entries.flatten().filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")).count();
    }
    println!("  Recorded WAL sessions: {}", session_count);

    let mut blob_count = 0;
    if let Ok(entries) = fs::read_dir(&blobs_dir) {
        blob_count = entries.flatten().count();
    }
    println!("  Stored large blobs: {}", blob_count);

    Ok(())
}

async fn cmd_ui(
    workspace: &Path,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("⚡ Booting Chassis sovereign microkernel for Web Dashboard...");
    let chassis_dir = workspace.join(".chassis");
    if !chassis_dir.exists() {
        eprintln!(
            "Error: .chassis directory not found in {}. Run 'chassis init' first.",
            workspace.display()
        );
        std::process::exit(1);
    }

    let policy_path = chassis_dir.join("security_policy.toml");
    let policy = if policy_path.exists() {
        let policy_str = fs::read_to_string(&policy_path)?;
        SecurityPolicy::from_toml(&policy_str)?
    } else {
        let mut p = SecurityPolicy::default();
        p.workspace.root = workspace.to_path_buf();
        p
    };

    let vault_path = get_vault_path(workspace);
    let unlocked_vault = if vault_path.exists() {
        if let Ok(pass) = get_vault_passphrase() {
            EncryptedVault::load_from_file(&vault_path, &pass).ok()
        } else {
            None
        }
    } else {
        None
    };

    let session_id = format!("ses_ui_{}", Utc::now().format("%Y%m%d_%H%M%S"));
    let sessions_dir = chassis_dir.join("sessions");
    let mut wal = WalWriter::init(&session_id, &sessions_dir)?;
    wal.append(
        "SESSION_START",
        json!({
            "workspace": workspace.display().to_string(),
            "mode": "web_ui"
        }),
    )?;
    let wal_path = sessions_dir.join(format!("{}.jsonl", session_id));

    let blobs_dir = chassis_dir.join("blobs");
    let blob_store = BlobStore::new(&blobs_dir)?;
    let router = Arc::new(CapabilityRouter::new(
        policy.clone(),
        workspace,
        blob_store,
        ExecutionMode::Interactive,
        None,
    ));
    let reverse_channel = router.create_reverse_channel();

    let mut plugins_to_boot = Vec::new();
    let search_roots = [
        workspace.join("crates/plugins"),
        chassis_dir.join("plugins"),
    ];

    for root in &search_roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let manifest_cand = entry.path().join("plugin.toml");
                if manifest_cand.is_file() && !plugins_to_boot.contains(&manifest_cand) {
                    plugins_to_boot.push(manifest_cand);
                }
            }
        }
    }

    let mut plugin_infos = Vec::new();

    for manifest_path in &plugins_to_boot {
        let manifest_content = fs::read_to_string(manifest_path)?;
        let manifest = PluginManifest::from_toml(&manifest_content)?;
        let plugin_id = manifest.plugin.id.clone();
        let plugin_version = manifest.plugin.version.clone();
        let plugin_dir = manifest_path.parent().unwrap_or(workspace);

        let caps_offered: Vec<PluginCapabilityInfo> = manifest
            .capabilities_offered
            .iter()
            .map(|c| PluginCapabilityInfo {
                id: c.id.clone(),
                methods: c.methods.clone(),
            })
            .collect();

        let perms = PluginPermissionsInfo {
            network: manifest.permissions.network.allow_outbound
                || !manifest.permissions.network.allowed_domains.is_empty(),
            filesystem: !manifest.permissions.filesystem.read_scopes.is_empty()
                || !manifest.permissions.filesystem.write_scopes.is_empty(),
        };

        match PluginSupervisor::launch_and_handshake_full(
            manifest,
            &policy,
            workspace,
            plugin_dir,
            &session_id,
            Duration::from_secs(5),
            unlocked_vault.as_ref(),
            Some(reverse_channel.clone()),
        )
        .await
        {
            Ok(supervisor) => {
                router.register_plugin(supervisor).await;
                wal.append(
                    "PLUGIN_BOOTED",
                    json!({
                        "plugin_id": plugin_id,
                        "capabilities": caps_offered.iter().map(|c| &c.id).collect::<Vec<_>>()
                    }),
                )?;
                plugin_infos.push(PluginInfo {
                    id: plugin_id,
                    version: plugin_version,
                    mode: "Isolated Process (Subreaper)".to_string(),
                    capabilities_offered: caps_offered,
                    permissions: perms,
                });
            }
            Err(e) => {
                eprintln!("   ⚠️ Failed to launch plugin {plugin_id}: {e}");
            }
        }
    }

    println!("✅ Microkernel ready: {} plugins online.", plugin_infos.len());

    let (tx_events, _) = broadcast::channel(200);

    let state = UiState {
        workspace: workspace.to_path_buf(),
        session_id,
        router,
        wal_path,
        plugins: plugin_infos,
        tx_events,
    };

    ui_server::start_ui_server(state, host, port).await?;
    Ok(())
}

