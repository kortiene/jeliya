//! Issue #160 — disposable Dioxus Android feasibility slice.
//!
//! This is intentionally a beachhead, not the future DirectClient. It calls
//! today's `jeliya-core::Engine` directly through one bounded serial actor so
//! the physical APK proves the process/dependency boundary without preserving
//! v1 as architecture. There is no Dart, `jeliya-ffi`, C ABI, socket, token, or
//! portfile on this path.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use dioxus::prelude::*;
use jeliya_core::{Engine, EngineConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

const STYLES: &str = include_str!("../assets/styles.css");
const SPIKE_STYLES: &str = r#"
:root { color-scheme: dark; }
html, body { min-height: 100%; background: #071017; color: #eaf2f4; }
body { margin: 0; font-family: system-ui, sans-serif; }
.spike-shell { min-height: 100vh; padding: max(1.25rem, env(safe-area-inset-top)) 1.25rem max(2rem, env(safe-area-inset-bottom)); box-sizing: border-box; }
.spike-kicker { color: #77e0c5; font-size: .78rem; font-weight: 800; letter-spacing: .12em; text-transform: uppercase; }
.spike-title { margin: .45rem 0 .6rem; font-size: clamp(1.8rem, 7vw, 3rem); line-height: 1.05; }
.spike-lede { max-width: 42rem; color: #b9cbd0; line-height: 1.55; }
.spike-status { margin: 1rem 0; padding: .85rem 1rem; border: 1px solid #2f5b5a; border-radius: .7rem; background: #0c2227; }
.spike-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr)); gap: .85rem; margin: 1rem 0; }
.spike-card { padding: 1rem; border: 1px solid #29424a; border-radius: .8rem; background: #0a171d; min-width: 0; }
.spike-card h2 { margin: 0 0 .75rem; font-size: 1rem; }
.spike-facts { display: grid; grid-template-columns: minmax(6rem, auto) 1fr; gap: .45rem .75rem; margin: 0; }
.spike-facts dt { color: #91a9af; }
.spike-facts dd { margin: 0; overflow-wrap: anywhere; }
.spike-actions { display: flex; flex-wrap: wrap; gap: .75rem; margin-top: 1rem; }
.spike-button { min-height: 3rem; padding: .65rem 1rem; border: 1px solid #77e0c5; border-radius: .65rem; background: #123632; color: #f4fffc; font: inherit; font-weight: 750; }
.spike-label { display: block; margin: 1.1rem 0 .4rem; font-weight: 700; }
.spike-input { width: 100%; min-height: 3rem; box-sizing: border-box; padding: .65rem .8rem; border: 1px solid #527078; border-radius: .55rem; background: #071017; color: white; font: inherit; }
.spike-list { margin: .4rem 0 0; padding-left: 1.25rem; }
.spike-ok { color: #9cf3d8; }
.spike-warn { color: #ffd18b; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: .82rem; }
"#;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct NativeSnapshot {
    state_path: Option<String>,
    webview_version: Option<String>,
    saf_status: Option<String>,
    saf_uri: Option<String>,
    saf_name: Option<String>,
    resume_count: u32,
    back_count: u32,
}

fn native_state() -> &'static Mutex<NativeSnapshot> {
    static STATE: OnceLock<Mutex<NativeSnapshot>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(NativeSnapshot::default()))
}

fn native_snapshot() -> NativeSnapshot {
    native_state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[derive(Clone, Debug, PartialEq)]
struct Bootstrap {
    state_path: String,
    marker_bytes: usize,
    identity_id: String,
    room_id: String,
    rooms: usize,
    endpoint_id: Option<String>,
    relay_observation: Option<String>,
    network_mode: String,
    serialized_calls: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Starting,
    Ready(Bootstrap),
    Failed(String),
}

#[derive(Clone)]
struct DirectActor {
    tx: mpsc::Sender<Command>,
}

struct Command {
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value, String>>,
}

impl DirectActor {
    async fn start(data_dir: PathBuf) -> Result<Self, String> {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        tokio::spawn(async move {
            let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<String>(1);
            let config = EngineConfig {
                port: 0,
                version: format!("{}-spike160", env!("CARGO_PKG_VERSION")),
                shutdown_tx,
            };

            // `false` is load-bearing: this is the real-network mode. It does
            // not imply a relay connected or a direct path exists.
            let engine = match Engine::new(data_dir, false, config) {
                Ok(engine) => engine,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("core start failed: {error}")));
                    return;
                }
            };
            let push_loop = engine.start_push_loop();
            let _ = ready_tx.send(Ok(()));

            loop {
                tokio::select! {
                    command = rx.recv() => {
                        let Some(command) = command else { break };
                        // Exactly one worker awaits exactly one dispatch at a
                        // time. Capacity 8 is bounded and QueueFull is visible
                        // to `enqueue`; this is only a beachhead for #173.
                        let result = engine
                            .dispatch(command.method, command.params)
                            .await
                            .map_err(|error| error.to_string());
                        let _ = command.reply.send(result);
                    }
                    _ = shutdown_rx.recv() => break,
                }
            }

            push_loop.stop();
            let _ = engine.close_all_rooms().await;
        });

        ready_rx
            .await
            .map_err(|_| "core worker ended before readiness".to_owned())??;
        Ok(Self { tx })
    }

    async fn call(&self, method: &'static str, params: Value) -> Result<Value, String> {
        let response = self.enqueue(method, params)?;
        response
            .await
            .map_err(|_| "DirectClient beachhead dropped a reply".to_owned())?
    }

    fn enqueue(
        &self,
        method: &'static str,
        params: Value,
    ) -> Result<oneshot::Receiver<Result<Value, String>>, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .try_send(Command {
                method,
                params,
                reply,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "QueueFull".to_owned(),
                mpsc::error::TrySendError::Closed(_) => "Stopped".to_owned(),
            })?;
        Ok(response)
    }
}

fn main() {
    dioxus::LaunchBuilder::mobile()
        .with_cfg(
            dioxus_desktop::Config::new()
                // Dioxus defaults to allow. The spike needs no external page.
                .with_navigation_handler(|_url| false)
                .with_disable_context_menu(true)
                .with_background_color((7, 16, 23, 255)),
        )
        .launch(App);
}

#[component]
fn App() -> Element {
    let mut phase = use_signal(|| Phase::Starting);
    let mut actor = use_signal(|| Option::<DirectActor>::None);
    let mut native = use_signal(native_snapshot);
    let mut viewport = use_signal(ViewportObservation::default);
    let mut last_resync = use_signal(|| "not requested".to_owned());

    use_future(move || async move {
        match boot().await {
            Ok((direct, bootstrap)) => {
                actor.set(Some(direct));
                phase.set(Phase::Ready(bootstrap));
            }
            Err(error) => phase.set(Phase::Failed(error)),
        }
    });

    // Observe Kotlin lifecycle/SAF callbacks without introducing a second IPC
    // system. Resume triggers a truthful authoritative room.list read; it is
    // not labeled reconnect because DirectClient never disconnected.
    use_future(move || async move {
        let mut seen_resume = 0;
        loop {
            let next = native_snapshot();
            if next.resume_count > seen_resume {
                seen_resume = next.resume_count;
                if let Some(direct) = actor.read().clone() {
                    last_resync.set(match direct.call("room.list", json!({})).await {
                        Ok(value) => format!(
                            "authoritative room.list after resume: {} room(s)",
                            value["rooms"].as_array().map_or(0, Vec::len)
                        ),
                        Err(error) => format!("resume resync failed: {error}"),
                    });
                }
            }
            native.set(next);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    // Rotation evidence is measured in the real system WebView, not inferred
    // from Android callbacks. Every resize reports viewport and orientation.
    use_future(move || async move {
        let mut eval = document::eval(
            r#"
            const report = () => dioxus.send({
              width: window.innerWidth,
              height: window.innerHeight,
              orientation: screen.orientation ? screen.orientation.type : "unavailable"
            });
            window.addEventListener("resize", report);
            report();
            await new Promise(() => {});
            "#,
        );
        while let Ok(observation) = eval.recv::<ViewportObservation>().await {
            viewport.set(observation);
        }
    });

    let phase_read = phase.read().clone();
    let native_read = native.read().clone();
    let viewport_read = viewport.read().clone();
    let resync_read = last_resync.read().clone();

    rsx! {
        style { "{STYLES}\n{SPIKE_STYLES}" }
        main { class: "spike-shell",
            p { class: "spike-kicker", "Dioxus Android · issue #160" }
            h1 { class: "spike-title", "Physical-device feasibility" }
            p { class: "spike-lede",
                "Disposable Dioxus 0.7 system-WebView slice. Native Rust calls jeliya-core in-process through one bounded serial actor. No Dart, jeliya-ffi, socket, token, or portfile is in this path."
            }

            match phase_read {
                Phase::Starting => rsx! {
                    div { class: "spike-status", role: "status", aria_live: "polite",
                        "Starting protected state and the real-network core…"
                    }
                },
                Phase::Failed(error) => rsx! {
                    div { class: "spike-status spike-warn", role: "alert",
                        strong { "Bootstrap failed. " }
                        span { class: "mono", "{error}" }
                    }
                },
                Phase::Ready(ref ready) => rsx! {
                    div { id: "bootstrap-status", class: "spike-status spike-ok", role: "status", aria_live: "polite",
                        "In-process bootstrap completed. Network mode is real; path reachability is not inferred."
                    }
                    section { class: "spike-grid", aria_label: "Bootstrap evidence",
                        article { class: "spike-card",
                            h2 { "Rust and storage" }
                            dl { class: "spike-facts",
                                dt { "boundary" } dd { id: "fact-boundary", "jeliya-core direct" }
                                dt { "state" } dd { id: "fact-state", class: "mono", "{ready.state_path}" }
                                dt { "test marker" } dd { "{ready.marker_bytes} bytes" }
                                dt { "serialized calls" } dd { id: "fact-calls", "{ready.serialized_calls}" }
                            }
                        }
                        article { class: "spike-card",
                            h2 { "Real-network observation" }
                            dl { class: "spike-facts",
                                dt { "mode" } dd { id: "fact-network-mode", "{ready.network_mode}" }
                                dt { "identity" } dd { class: "mono", "{ready.identity_id}" }
                                dt { "room" } dd { class: "mono", "{ready.room_id}" }
                                dt { "rooms" } dd { "{ready.rooms}" }
                                dt { "endpoint" } dd { id: "fact-endpoint", class: "mono", {ready.endpoint_id.as_deref().unwrap_or("not reported")} }
                                dt { "relay field" } dd { id: "fact-relay", class: "mono", {ready.relay_observation.as_deref().unwrap_or("not reported")} }
                            }
                        }
                        article { class: "spike-card",
                            h2 { "Device lifecycle" }
                            dl { class: "spike-facts",
                                dt { "WebView" } dd { id: "fact-webview", {native_read.webview_version.as_deref().unwrap_or("waiting")} }
                                dt { "viewport" } dd { id: "fact-viewport", "{viewport_read.width} × {viewport_read.height}" }
                                dt { "orientation" } dd { id: "fact-orientation", "{viewport_read.orientation}" }
                                dt { "resume callbacks" } dd { id: "fact-resumes", "{native_read.resume_count}" }
                                dt { "resume action" } dd { id: "fact-resync", "{resync_read}" }
                                dt { "Back callbacks" } dd { id: "fact-backs", "{native_read.back_count}" }
                            }
                        }
                    }
                },
            }

            section { class: "spike-card", aria_label: "Interaction probes",
                h2 { "IME, file picker, and accessibility probes" }
                label { class: "spike-label", r#for: "composer-probe", "Message field" }
                input {
                    id: "composer-probe",
                    class: "spike-input",
                    r#type: "text",
                    inputmode: "text",
                    autocomplete: "off",
                    placeholder: "Tap to show the keyboard",
                    aria_label: "Message field for keyboard test",
                }
                div { class: "spike-actions",
                    button {
                        id: "saf-button",
                        class: "spike-button",
                        r#type: "button",
                        onclick: move |_| {
                            if let Err(error) = android_bridge::launch_saf_picker() {
                                let mut current = native_state()
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                current.saf_status = Some(format!("launch error: {error}"));
                            }
                        },
                        "Choose a test file"
                    }
                }
                p { id: "saf-status", role: "status", aria_live: "polite",
                    "Picker: "
                    {native_read.saf_status.as_deref().unwrap_or("not opened")}
                }
                if let Some(name) = native_read.saf_name.as_deref() {
                    p { "Selected display name: ", span { id: "saf-name", class: "mono", "{name}" } }
                }
                if let Some(uri) = native_read.saf_uri.as_deref() {
                    p { "Content URI (not treated as a path): ", span { id: "saf-uri", class: "mono", "{uri}" } }
                }
                ul { class: "spike-list",
                    li { "Heading and landmark semantics for TalkBack navigation" }
                    li { "Live regions for bootstrap, picker, and lifecycle state" }
                    li { "48dp-class touch targets and an explicitly labeled text field" }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct ViewportObservation {
    width: u32,
    height: u32,
    #[serde(default)]
    orientation: String,
}

async fn boot() -> Result<(DirectActor, Bootstrap), String> {
    let state_path = android_bridge::prepare_protected_state()?;
    let state = PathBuf::from(&state_path);
    verify_spike_state(&state)?;
    let marker_bytes = std::fs::metadata(state.join("spike-test-data.json"))
        .map_err(|error| format!("test marker unavailable: {error}"))?
        .len() as usize;

    let direct = DirectActor::start(state.clone()).await?;
    let mut calls = 0_u64;

    let mut status = direct.call("daemon.status", json!({})).await?;
    calls += 1;
    let identity_id = match status["identity"]["identity_id"].as_str() {
        Some(identity) => identity.to_owned(),
        None => {
            let created = direct.call("identity.create", json!({})).await?;
            calls += 1;
            created["identity_id"]
                .as_str()
                .ok_or("identity.create omitted identity_id")?
                .to_owned()
        }
    };

    let listed = direct.call("room.list", json!({})).await?;
    calls += 1;
    let room_id = match listed["rooms"]
        .as_array()
        .and_then(|rooms| rooms.first())
        .and_then(|room| room["room_id"].as_str())
    {
        Some(room) => room.to_owned(),
        None => {
            let created = direct
                .call("room.create", json!({ "name": "Android M0 test room" }))
                .await?;
            calls += 1;
            created["room_id"]
                .as_str()
                .ok_or("room.create omitted room_id")?
                .to_owned()
        }
    };

    // This starts the actual iroh node with loopback=false. We record only the
    // endpoint fields the engine itself returns. No relay/direct success is
    // inferred from a URL, room membership, latency, or absence of an error.
    direct
        .call("room.open", json!({ "room_id": room_id, "peers": [] }))
        .await?;
    calls += 1;
    status = direct.call("daemon.status", json!({})).await?;
    calls += 1;
    let final_rooms = direct.call("room.list", json!({})).await?;
    calls += 1;

    let endpoint_id = status["endpoint"]["endpoint_id"]
        .as_str()
        .map(str::to_owned);
    let relay_observation = match status["endpoint"].get("relay_url") {
        Some(Value::String(url)) => {
            Some(format!("engine reported URL: {url}; connectivity unproven"))
        }
        Some(Value::Null) | None => Some("engine reported no relay URL".to_owned()),
        Some(other) => Some(format!("unexpected engine field: {other}")),
    };
    let rooms = final_rooms["rooms"].as_array().map_or(0, Vec::len);
    let network_mode = status["mode"].as_str().unwrap_or("unknown").to_owned();

    let evidence = json!({
        "boundary": "jeliya-core direct",
        "ffi": false,
        "dart": false,
        "network_mode": network_mode,
        "endpoint_id_reported": endpoint_id.is_some(),
        "relay_observation": relay_observation,
        "serialized_calls": calls,
        "test_data": true,
    });
    std::fs::write(
        state.join("native-evidence.json"),
        serde_json::to_vec_pretty(&evidence).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("could not write native evidence: {error}"))?;

    eprintln!("SPIKE160_NATIVE {evidence}");
    Ok((
        direct,
        Bootstrap {
            state_path,
            marker_bytes,
            identity_id,
            room_id,
            rooms,
            endpoint_id,
            relay_observation,
            network_mode,
            serialized_calls: calls,
        },
    ))
}

fn verify_spike_state(path: &Path) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("state path is not canonicalizable: {error}"))?;
    if canonical.file_name().and_then(|name| name.to_str()) != Some("dioxus-m0-spike-v1") {
        return Err("native service returned an unrecognized state generation".to_owned());
    }
    if !canonical.is_dir() {
        return Err("native service returned a non-directory state path".to_owned());
    }
    let marker = canonical.join("spike-test-data.json");
    let parsed: Value = serde_json::from_slice(
        &std::fs::read(&marker).map_err(|error| format!("test marker unreadable: {error}"))?,
    )
    .map_err(|error| format!("test marker invalid: {error}"))?;
    if parsed["generation"] != "dioxus-m0-spike-v1" || parsed["test_data"] != true {
        return Err("protected marker is not this spike's test generation".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_state_requires_the_exact_generation_marker() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("dioxus-m0-spike-v1");
        std::fs::create_dir(&state).unwrap();
        assert!(
            verify_spike_state(&state).is_err(),
            "unmarked state was adopted"
        );

        std::fs::write(
            state.join("spike-test-data.json"),
            br#"{"generation":"wrong","test_data":true}"#,
        )
        .unwrap();
        assert!(
            verify_spike_state(&state).is_err(),
            "wrong generation was adopted"
        );

        std::fs::write(
            state.join("spike-test-data.json"),
            br#"{"generation":"dioxus-m0-spike-v1","test_data":true}"#,
        )
        .unwrap();
        verify_spike_state(&state).expect("exact test generation is accepted");
    }

    #[tokio::test]
    async fn bounded_actor_surfaces_queue_full() {
        let (tx, _rx) = mpsc::channel::<Command>(1);
        let actor = DirectActor { tx };
        let first = actor.enqueue("room.list", json!({}));
        assert!(first.is_ok());
        assert_eq!(
            actor.enqueue("room.list", json!({})).unwrap_err(),
            "QueueFull"
        );
    }
}

#[cfg(target_os = "android")]
mod android_bridge {
    use super::*;
    use jni::objects::{JObject, JString};
    use jni::{JNIEnv, JavaVM};

    fn with_activity<T>(
        operation: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        let context = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }
            .map_err(|error| format!("Java VM unavailable: {error}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|error| format!("JNI attach failed: {error}"))?;
        let activity = unsafe { JObject::from_raw(context.context().cast()) };
        operation(&mut env, &activity)
    }

    pub fn prepare_protected_state() -> Result<String, String> {
        with_activity(|env, activity| {
            let value = env
                .call_method(
                    activity,
                    "prepareProtectedState",
                    "()Ljava/lang/String;",
                    &[],
                )
                .and_then(|value| value.l())
                .map_err(|error| format!("protected-state JNI call failed: {error}"))?;
            let value = JString::from(value);
            env.get_string(&value)
                .map(|text| text.into())
                .map_err(|error| format!("protected-state JNI result failed: {error}"))
        })
    }

    pub fn launch_saf_picker() -> Result<(), String> {
        with_activity(|env, activity| {
            env.call_method(activity, "launchSafPicker", "()V", &[])
                .map(|_| ())
                .map_err(|error| format!("SAF JNI call failed: {error}"))
        })
    }

    fn string(env: &mut JNIEnv<'_>, value: JString<'_>) -> String {
        env.get_string(&value)
            .map(|text| text.into())
            .unwrap_or_default()
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativePlatformReady(
        mut env: JNIEnv,
        _activity: JObject,
        state_path: JString,
        webview_version: JString,
    ) {
        let path = string(&mut env, state_path);
        let version = string(&mut env, webview_version);
        let mut state = native_state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.state_path = Some(path.clone());
        state.webview_version = Some(version.clone());
        eprintln!("SPIKE160_PLATFORM_READY state={path} webview={version}");
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeSafResult(
        mut env: JNIEnv,
        _activity: JObject,
        status: JString,
        uri: JString,
        display_name: JString,
    ) {
        let status = string(&mut env, status);
        let uri = string(&mut env, uri);
        let display_name = string(&mut env, display_name);
        let mut state = native_state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.saf_status = Some(status.clone());
        state.saf_uri = (!uri.is_empty()).then_some(uri);
        state.saf_name = (!display_name.is_empty()).then_some(display_name.clone());
        // Do not log the URI or user-selected filename; the manual evidence
        // uses an explicitly harmless file and captures the rendered result.
        eprintln!("SPIKE160_SAF status={status}");
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeResumed(
        _env: JNIEnv,
        _activity: JObject,
    ) {
        let mut state = native_state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.resume_count = state.resume_count.saturating_add(1);
        eprintln!("SPIKE160_RESUME count={}", state.resume_count);
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeBackInvoked(
        _env: JNIEnv,
        _activity: JObject,
    ) {
        let mut state = native_state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.back_count = state.back_count.saturating_add(1);
        eprintln!("SPIKE160_BACK count={}", state.back_count);
    }
}

#[cfg(not(target_os = "android"))]
mod android_bridge {
    pub fn prepare_protected_state() -> Result<String, String> {
        Err("this disposable slice must run as an Android artifact".to_owned())
    }

    pub fn launch_saf_picker() -> Result<(), String> {
        Err("SAF is only available in the physical Android artifact".to_owned())
    }
}
