//! The native rig: `WsNative` (tokio + tokio-tungstenite) against a REAL
//! `jeliyad` spawned by the rig on an OS-assigned loopback port. The source
//! resolver re-reads the daemon's portfile on every resolve, so a
//! `restart_backend` (respawn on a NEW port from the SAME data dir) is
//! survived by the adapter's reconnect loop — exactly the contract C9 pins.
//!
//! A missing `jeliyad` binary FAILS the rig (never skips): a missing oracle
//! must not read as a passed contract.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use jeliya_client::contract::{ContractFailure, ContractResult, Rig, RigConfig};
use jeliya_client::{connect_ws_native, KernelConfig, KernelLimits, NativeClientConfig, TickDelta};
use jeliya_client::{ClientHandle, Dial, DialResolveError, TargetSource};

pub struct NativeRig {
    config: RigConfig,
    handle: ClientHandle,
    daemon: Daemon,
}

struct Daemon {
    child: Child,
    dir: PathBuf,
}

impl Daemon {
    fn portfile(&self) -> PathBuf {
        self.dir.join("daemon.json")
    }

    fn spawn(config: &RigConfig) -> Result<Self, ContractFailure> {
        if !config.jeliyad_bin.is_file() {
            return Err(ContractFailure(format!(
                "[native] jeliyad not found at {} — build it (cargo build -p jeliyad) or set JELIYAD_BIN; a missing oracle fails rather than skips",
                config.jeliyad_bin.display()
            )));
        }
        std::fs::create_dir_all(&config.data_dir).map_err(|e| {
            ContractFailure(format!(
                "[native] could not create {}: {e}",
                config.data_dir.display()
            ))
        })?;
        let child = Command::new(&config.jeliyad_bin)
            .arg("--port")
            .arg("0")
            .arg("--data-dir")
            .arg(&config.data_dir)
            .arg("--no-open")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                ContractFailure(format!(
                    "[native] could not spawn {}: {e}",
                    config.jeliyad_bin.display()
                ))
            })?;
        let daemon = Self {
            child,
            dir: config.data_dir.clone(),
        };
        daemon.wait_portfile()?;
        Ok(daemon)
    }

    /// Poll until the portfile exists and carries a port, token, and
    /// storage generation (bounded; jeliyad writes it at startup).
    fn wait_portfile(&self) -> Result<(), ContractFailure> {
        let deadline = Duration::from_secs(20);
        let start = std::time::Instant::now();
        loop {
            if let Ok(facts) = read_portfile(&self.portfile()) {
                let _ = facts;
                return Ok(());
            }
            if start.elapsed() > deadline {
                return Err(ContractFailure(format!(
                    "[native] daemon did not write a valid portfile within 20 s (pid {})",
                    self.child.id()
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn is_alive(&mut self) -> bool {
        matches!(
            self.child.try_wait(),
            Ok(None) // no exit status yet → still running
        )
    }
}

// NOTE: Daemon has no Drop on purpose. `restart_backend` replaces the
// `Daemon` value, and a Drop here would delete the data dir — the very
// durable state the restart contract asserts. Teardown (kill + remove the
// temp dir) belongs to the RIG's drop, which runs only when the whole test
// is done with the directory.

/// The verified facts a portfile carries.
struct PortfileFacts {
    port: u16,
    token: String,
    storage_generation: u64,
}

fn read_portfile(path: &Path) -> Result<PortfileFacts, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let port = value["port"].as_u64().ok_or("port")? as u16;
    let generation = value["storage_generation"]
        .as_u64()
        .ok_or("storage_generation")?;
    // auth_token may be a plain string or a Redacted<T> {"inner": "..."}.
    let token = match &value["auth_token"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map
            .get("inner")
            .and_then(|v| v.as_str())
            .ok_or("inner")?
            .to_owned(),
        _ => return Err("auth_token".into()),
    };
    if token.is_empty() {
        return Err("empty auth_token".into());
    }
    Ok(PortfileFacts {
        port,
        token,
        storage_generation: generation,
    })
}

/// A [`TargetSource`] that re-reads the daemon's portfile on EVERY resolve,
/// so a respawned daemon (new port, new storage generation fence) is picked
/// up by the adapter's reconnect loop without rebuilding the handle.
struct PortfileSource {
    path: PathBuf,
    version: u64,
    /// The declared client id: a STABLE session principal across reconnects
    /// and daemon restarts, exactly as a real desktop client declares one.
    /// Without it the gate mints a fresh ephemeral principal per connection
    /// and a restarted client would see none of its own rooms.
    cid: String,
}

impl TargetSource for PortfileSource {
    fn resolve(&self) -> futures::future::BoxFuture<'static, Result<Dial, DialResolveError>> {
        let path = self.path.clone();
        let version = self.version;
        let cid = self.cid.clone();
        Box::pin(async move {
            let facts = read_portfile(&path)
                .map_err(|e| DialResolveError::Transient(format!("portfile unreadable: {e}")))?;
            let url = url::Url::parse(&format!(
                "ws://127.0.0.1:{port}/ws?v={version}&sg={sg}&cid={cid}",
                port = facts.port,
                sg = facts.storage_generation,
                cid = cid
            ))
            .map_err(|e| DialResolveError::Transient(format!("url: {e}")))?;
            Ok(Dial {
                url,
                bearer: jeliya_supervisor::Redacted::new(facts.token),
            })
        })
    }
}

fn kernel_limits(config: &RigConfig) -> KernelLimits {
    KernelLimits {
        queue_depth: config.queue_depth,
        in_flight: config.in_flight,
        max_reconnect_attempts: config.max_reconnect_attempts,
        backoff_base: TickDelta::from_ticks(config.backoff_base_ms),
        backoff_cap: TickDelta::from_ticks(config.backoff_cap_ms),
        ..KernelLimits::default()
    }
}

impl Drop for NativeRig {
    fn drop(&mut self) {
        // The rig — not the Daemon — owns teardown: restart_backend replaces
        // the Daemon value mid-test, and only the END of the test may remove
        // the data dir.
        self.daemon.kill();
        let _ = std::fs::remove_dir_all(&self.config.data_dir);
    }
}

impl Rig for NativeRig {
    fn name(&self) -> &'static str {
        "native"
    }

    fn handle(&self) -> &ClientHandle {
        &self.handle
    }

    fn config(&self) -> &RigConfig {
        &self.config
    }

    async fn spawn(config: RigConfig) -> Result<Self, ContractFailure> {
        let daemon = Daemon::spawn(&config)?;
        let source = PortfileSource {
            path: daemon.portfile(),
            version: config.gate_version,
            cid: format!("jeliya-175-{}", config.label),
        };
        let native = NativeClientConfig {
            kernel: KernelConfig {
                limits: kernel_limits(&config),
                ..KernelConfig::default()
            },
            connect_timeout: Duration::from_millis(config.connect_timeout_ms),
            hello_timeout: Duration::from_millis(config.hello_timeout_ms),
        };
        let handle = connect_ws_native(source, native).map_err(|e| {
            ContractFailure(format!(
                "[native] connect_ws_native refused the config: {e}"
            ))
        })?;
        Ok(Self {
            config,
            handle,
            daemon,
        })
    }

    fn sleep_ms(&self, ms: u64) -> impl Future<Output = ()> {
        tokio::time::sleep(Duration::from_millis(ms))
    }

    async fn on_ready(&mut self) -> ContractResult {
        // The daemon answers subject-gated mutations with `subject_absent`
        // until the local subject exists — the corpus stages the same
        // `subject.ensure` before room operations. When the config parks
        // every call (`in_flight = 0`), identity is unobservable (nothing
        // ever leaves the seam), so the ensure is skipped rather than
        // parked forever.
        //
        // Right after a reconnect or a daemon restart the connection can
        // still be mid-cycle; `subject.ensure` is naturally idempotent, so
        // a settlement that proves nothing executed (`Disconnected`, any
        // `Cancelled`) is retried here — TYPED, not string-matched, and
        // bounded. Every other verdict is final and surfaces as a failure.
        if self.config.in_flight == 0 {
            return Ok(());
        }
        let (polls, poll_ms) = (self.config.wait_polls, self.config.poll_ms);
        for _ in 0..polls {
            match self
                .handle
                .call::<jeliya_api::SubjectEnsure>(
                    jeliya_api::SubjectEnsure {},
                    jeliya_client::Dedup::None,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(
                    e @ (jeliya_client::CallError::Disconnected { .. }
                    | jeliya_client::CallError::Cancelled { .. }),
                ) => {
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(poll_ms)).await;
                }
                Err(e) => {
                    return Err(ContractFailure(format!(
                        "[native] subject.ensure failed: {e:?}"
                    )))
                }
            }
        }
        Err(ContractFailure(
            "[native] subject.ensure kept settling as connection-cycled within the budget"
                .to_owned(),
        ))
    }

    async fn sever_transport(&mut self) -> ContractResult {
        // SIGKILL: the socket dies with no close frame — the exact
        // transport-loss shape the contract pins.
        if !self.daemon.is_alive() {
            return Err(ContractFailure(
                "[native] daemon already dead before sever_transport".to_owned(),
            ));
        }
        self.daemon.kill();
        Ok(())
    }

    async fn restart_backend(&mut self) -> ContractResult {
        if self.daemon.is_alive() {
            self.daemon.kill();
        }
        // Same data dir → same durable rooms; new port + fresh token → the
        // resolver must be re-read for the client to come back, which is
        // exactly what C9 pins. The stale portfile is left in place: dials
        // against the dead port fail transiently under backoff, while a
        // MISSING file makes every resolve fail instantly and would burn
        // the whole attempt budget before the daemon rewrites it.
        self.daemon = Daemon::spawn(&self.config)?;
        Ok(())
    }
}
