//! Issue #159 — a disposable Dioxus desktop shell that supervises or adopts
//! `jeliyad`, talks to it from native Rust, and renders through the system
//! WebView (webkit2gtk on Linux).
//!
//! Read `README.md` before trusting anything here. The short version:
//!
//! - the daemon token lives in the native half and is never handed to the
//!   WebView, never rendered, never logged;
//! - navigation is fail-closed because Dioxus's default fails OPEN;
//! - downloads and new-window policy are NOT closed, because `dioxus-desktop`
//!   0.7.9 exposes no hook for them. That is a negative result, recorded rather
//!   than papered over.

mod client;
mod supervisor;

use dioxus::prelude::*;

/// The daemon's own stylesheet, embedded byte-identically. The `#158` spike
/// proved `ui/src/styles.css` survives a renderer swap; this one only needs to
/// prove it survives the *desktop* renderer too, so it reuses the same bytes
/// rather than a hand-written copy.
const STYLES: &str = include_str!("../assets/styles.css");

/// What the native half has learned, in a form safe to render. Note what is
/// NOT here: the token. This struct is the boundary — if a field cannot appear
/// in a screenshot, it does not belong in it.
#[derive(Clone, Debug, PartialEq)]
struct Bootstrap {
    ownership: &'static str,
    pid: u32,
    ws_url: String,
    protocol: u32,
    version: String,
    rooms: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Starting,
    Ready(Bootstrap),
    Failed(String),
}

fn main() {
    // webkit2gtk's DMABUF renderer fails on this host's driver and the WebView
    // then renders NOTHING — a window appears, the process is healthy, and the
    // page is blank. Dioxus does set this variable, but only when
    // XDG_SESSION_TYPE == "wayland"; on X11 it does not, while also forcing
    // GDK_BACKEND=x11 unconditionally. So an X11 session gets the broken path.
    //
    // Set before any GTK/WebKit initialisation, i.e. before launch().
    // SAFETY: single-threaded, before any other thread exists.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    let config = dioxus_desktop::Config::new()
        .with_window(
            dioxus_desktop::WindowBuilder::new()
                .with_title("Jeliya desktop spike (#159)")
                .with_inner_size(dioxus_desktop::LogicalSize::new(1100.0, 720.0)),
        )
        // FAIL CLOSED. Dioxus's default navigation handler returns true, i.e.
        // it permits navigation anywhere — including an http(s) origin, which
        // would replace the trusted document with a remote one inside a WebView
        // that shares this process. Nothing in this shell needs to navigate.
        .with_navigation_handler(|_url| false)
        .with_menu(None);

    dioxus_desktop::launch::launch(App, Vec::new(), vec![Box::new(config)]);
}

#[component]
fn App() -> Element {
    let phase = use_signal(|| Phase::Starting);

    // The native kernel. It owns the Sidecar (and therefore the token) for the
    // whole life of the app and publishes only `Bootstrap` into the UI.
    use_future(move || {
        let mut phase = phase;
        async move {
            match boot().await {
                Ok((sidecar, bootstrap)) => {
                    phase.set(Phase::Ready(bootstrap));
                    // Hold the sidecar forever: dropping it would drop the
                    // child's stdin and end an owned daemon under our own feet.
                    std::mem::forget(sidecar);
                }
                Err(why) => phase.set(Phase::Failed(why)),
            }
        }
    });

    rsx! {
        style { "{STYLES}" }
        main { class: "app pane-room",
            match &*phase.read() {
                Phase::Starting => rsx! {
                    section { class: "boot-screen",
                        h1 { class: "boot-target", "Starting jeliyad…" }
                    }
                },
                Phase::Failed(why) => rsx! {
                    section { class: "boot-screen",
                        div { class: "error-note",
                            h1 { class: "error-title", "Could not reach a daemon" }
                            p { class: "error-code mono", "{why}" }
                        }
                    }
                },
                Phase::Ready(b) => rsx! {
                    RenderProbe {}
                    section { class: "center",
                        h1 { id: "bootstrap-heading", class: "center-empty-title",
                            "Connected to a {b.ownership} daemon"
                        }
                        dl { id: "bootstrap-facts", class: "mono",
                            dt { "pid" }        dd { id: "fact-pid", "{b.pid}" }
                            dt { "endpoint" }   dd { id: "fact-ws", "{b.ws_url}" }
                            dt { "protocol" }   dd { id: "fact-protocol", "{b.protocol}" }
                            dt { "version" }    dd { id: "fact-version", "{b.version}" }
                            dt { "rooms" }      dd { id: "fact-rooms", "{b.rooms}" }
                        }
                    }
                },
            }
        }
    }
}

/// Headless render evidence, enabled by `SPIKE_RENDER_PROBE=1`.
///
/// A window that opens proves nothing: webkit2gtk's DMABUF path fails on this
/// host by rendering a perfectly healthy BLANK page. So the probe measures what
/// the WebView actually laid out — `getBoundingClientRect` and
/// `getComputedStyle` — and reports it back into Rust, which prints one JSON
/// line and exits. An unstyled or unrendered page fails the assertions.
///
/// It also proves a negative that no screenshot could: the daemon token is
/// absent from the entire serialized DOM.
#[component]
fn RenderProbe() -> Element {
    use_future(move || async move {
        if std::env::var("SPIKE_RENDER_PROBE").as_deref() != Ok("1") {
            return;
        }
        // Give the WebView a frame to lay out before measuring.
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;

        let result = dioxus::document::eval(
            r#"
            const heading = document.getElementById('bootstrap-heading');
            const facts = document.getElementById('bootstrap-facts');
            if (!heading || !facts) { return JSON.stringify({ error: 'nodes missing' }); }
            const hr = heading.getBoundingClientRect();
            const cs = getComputedStyle(heading);
            return JSON.stringify({
              heading_width: hr.width,
              heading_height: hr.height,
              heading_text: heading.textContent,
              // Proof the stylesheet applied rather than browser defaults:
              // .center-empty-title sets an explicit weight and size.
              font_weight: cs.fontWeight,
              font_size: cs.fontSize,
              body_bg: getComputedStyle(document.body).backgroundColor,
              pid_text: (document.getElementById('fact-pid')||{}).textContent,
              dom_length: document.documentElement.outerHTML.length,
              dom: document.documentElement.outerHTML,
            });
            "#,
        )
        .await;

        match result {
            Ok(value) => {
                // `eval` hands back whatever the script returned, and the
                // script returns a JSON *string* — so this arrives as
                // Value::String, not an object. Parse it once before touching
                // any field, or every `get` silently returns None.
                let mut value = match value.as_str() {
                    Some(text) => serde_json::from_str(text).unwrap_or(value),
                    None => value,
                };
                // The full DOM goes to a file, never to stdout: the harness
                // greps it for the token, and a 144 KB blob in a log is both
                // useless and the kind of place secrets get copied to.
                if let Some(path) = std::env::var_os("SPIKE_PROBE_DOM") {
                    if let Some(dom) = value.get("dom").and_then(serde_json::Value::as_str) {
                        let _ = std::fs::write(path, dom);
                    }
                }
                if let Some(object) = value.as_object_mut() {
                    object.remove("dom");
                }
                println!("SPIKE_RENDER_PROBE {value}");
            }
            Err(err) => println!("SPIKE_RENDER_PROBE {{\"error\":\"{err:?}\"}}"),
        }
        // Deadman: the probe either reported or the process is killed by the
        // caller's timeout. Exiting here keeps the harness simple.
        std::process::exit(0);
    });
    rsx! {}
}

/// Start or adopt a daemon, connect natively, and gather the facts worth
/// rendering. Returns the sidecar so the caller can keep it alive.
async fn boot() -> Result<(supervisor::Sidecar, Bootstrap), String> {
    let binary = supervisor::resolve_jeliyad().map_err(|e| e.to_string())?;
    let data_dir = std::env::var_os("JELIYA_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("jeliya-spike-159"));

    let sidecar = supervisor::start_or_adopt(&binary, &data_dir)
        .await
        .map_err(|e| e.to_string())?;

    let mut connection = client::Connection::connect(sidecar.ws_url(), sidecar.token())
        .await
        .map_err(|e| e.to_string())?;

    let status = connection.check_protocol().await.map_err(|e| e.to_string())?;
    let rooms = connection
        .call("room.list", serde_json::json!({}))
        .await
        .map_err(|e| e.to_string())?;

    let bootstrap = Bootstrap {
        ownership: if sidecar.is_owned() { "supervised" } else { "adopted" },
        pid: sidecar.portfile.pid,
        ws_url: sidecar.ws_url().to_string(),
        protocol: status.get("protocol").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32,
        version: sidecar.portfile.version.clone(),
        rooms: rooms
            .get("rooms")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
    };
    Ok((sidecar, bootstrap))
}
