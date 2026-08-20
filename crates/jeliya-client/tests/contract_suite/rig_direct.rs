//! The direct rig: `DirectClient` (#173) over a REAL typed `Engine`
//! in-process — no daemon, no socket, no token. Persistence is the engine's
//! own data dir, so C9's restart leg is a full stop → rebuild → re-read of
//! the same home. There is no transport to sever and no wire gate to refuse
//! a version: those contracts are N/A here with recorded reasons.

use std::future::Future;
use std::time::Duration;

use jeliya_client::contract::{ContractFailure, ContractResult, Rig, RigConfig};
use jeliya_client::{ClientHandle, DirectConfig};
use jeliya_client::{Dedup, KernelLimits, TickDelta};

pub struct DirectRig {
    config: RigConfig,
    handle: ClientHandle,
}

impl Drop for DirectRig {
    fn drop(&mut self) {
        // The rig owns its temp data dir; nothing else references it.
        let _ = std::fs::remove_dir_all(&self.config.data_dir);
    }
}

fn direct_config(config: &RigConfig) -> DirectConfig {
    let mut cfg = DirectConfig::new(config.data_dir.clone());
    cfg.limits = KernelLimits {
        queue_depth: config.queue_depth,
        in_flight: config.in_flight,
        max_reconnect_attempts: config.max_reconnect_attempts,
        backoff_base: TickDelta::from_ticks(config.backoff_base_ms),
        backoff_cap: TickDelta::from_ticks(config.backoff_cap_ms),
        ..KernelLimits::default()
    };
    cfg
}

impl DirectRig {
    /// Provision the local subject with a SEPARATE default-limits handle:
    /// the contract handle may be configured to park every call
    /// (`nothing_ever_sends`), which would park the identity call too.
    /// `subject.ensure` is idempotent and the store persists it, so the
    /// contract handle inherits a provisioned engine.
    async fn provision_subject(config: &RigConfig) -> Result<(), ContractFailure> {
        let mut defaults = RigConfig::new(
            &format!("{}-provision", config.label),
            config.data_dir.clone(),
            config.jeliyad_bin.clone(),
        );
        defaults.label = format!("{}-provision", config.label);
        let handle = jeliya_client::connect_direct(direct_config(&defaults));
        handle.start();
        wait_ready(&handle).await?;
        let ensured = handle
            .call::<jeliya_api::SubjectEnsure>(jeliya_api::SubjectEnsure {}, Dedup::None)
            .await;
        handle.stop().await;
        ensured.map(|_| ()).map_err(|e| {
            ContractFailure(format!(
                "[direct] provisioning subject.ensure failed: {e:?}"
            ))
        })
    }
}

/// Poll a handle to Ready (bounded, poll-counted).
async fn wait_ready(handle: &jeliya_client::ClientHandle) -> Result<(), ContractFailure> {
    for _ in 0..400 {
        if handle.state() == jeliya_client::State::Ready {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    Err(ContractFailure(
        "[direct] provisioning handle never reached Ready".to_owned(),
    ))
}

impl Rig for DirectRig {
    fn name(&self) -> &'static str {
        "direct"
    }

    fn handle(&self) -> &ClientHandle {
        &self.handle
    }

    fn config(&self) -> &RigConfig {
        &self.config
    }

    async fn spawn(config: RigConfig) -> Result<Self, ContractFailure> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| {
            ContractFailure(format!(
                "[direct] could not create {}: {e}",
                config.data_dir.display()
            ))
        })?;
        Self::provision_subject(&config).await?;
        let handle = jeliya_client::connect_direct(direct_config(&config));
        Ok(Self { config, handle })
    }

    fn sleep_ms(&self, ms: u64) -> impl Future<Output = ()> {
        tokio::time::sleep(Duration::from_millis(ms))
    }

    fn supports_stream_subscribe(&self) -> bool {
        // The direct actor routes stream.* to the engine dispatcher, which
        // answers subscription_unknown (jeliya-core/src/typed.rs — the
        // dispatcher explicitly errors StreamSubscribe/Unsubscribe/Resync).
        // A per-connection subscription surface for the direct adapter is
        // an open follow-up, not a contract waiver; C3 rides the engine's
        // own push forwarding here.
        false
    }

    async fn restart_backend(&mut self) -> ContractResult {
        // Graceful stop, then a brand-new engine over the SAME home. What
        // the old engine committed must still be readable — the engine's
        // store persists rooms under the data dir, exactly as the daemon's
        // does.
        self.handle.stop().await;
        self.handle = jeliya_client::connect_direct(direct_config(&self.config));
        self.handle.start();
        Ok(())
    }
}
