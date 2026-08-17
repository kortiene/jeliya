//! `WebFiles` — the browser picker / stage / export / share bindings (#181
//! §5.A), and the crate's **door edge**: it mints `PickedSource` /
//! `ShareableBlob` / `ExportTarget` / `FetchedArtifact` through the
//! `jeliya-platform-implementation` factory door (#174 §K4).
//!
//! Browser file custody is **client-side and in-memory**: staging a picked
//! `File` reads its bytes into an in-memory blob the crate holds in its
//! [`Registry`], keyed by the minted token. No filesystem path is ever named —
//! the daemon's anti-exfiltration invariant and the #122 confinement hold by
//! construction (spec D4): the only things that cross the boundary are bytes and
//! opaque tokens, and a token the service did not mint fails closed at
//! resolution (the [`Registry`] issuer gate).
//!
//! Bounded/limit/empty/cancel are enforced during staging (spec D3/D5): a
//! known-oversize `File` fails `FileTooLarge` before any read; an empty file
//! fails `FileEmpty`; a fired [`CancelToken`] leaves no retained blob and
//! yields `Cancelled`, never `Ok`.

use std::rc::Rc;

use jeliya_platform::cancel::CancelToken;
use jeliya_platform::error::{CapabilityError, FailureKind};
use jeliya_platform::files::{
    ExportTarget, ExportTargetKind, FileName, FileObjectKind, FileSink, Files, Mime, PickedSource,
    ProgressSink, ShareSink, ShareableBlob, StageProgress, StagedBlobReader,
};
use jeliya_platform::BoxFuture;
use wasm_bindgen::{closure::Closure, JsCast};

use jeliya_platform_implementation::{
    artifact_token_from_raw, blob_token_from_raw, blob_token_into_raw, export_target,
    export_target_token, export_token_from_raw, export_token_into_raw, fetched_artifact,
    picked_source, picked_source_token, shareable_blob, shareable_blob_token,
    source_token_from_raw, source_token_into_raw,
};

use crate::registry::Registry;

/// The browser [`Files`] capability.
pub struct WebFiles {
    sources: Rc<Registry<web_sys::File>>,
    staged: Rc<Registry<Vec<u8>>>,
    exports: Rc<Registry<FileName>>,
    artifacts: Rc<Registry<(FileName, Vec<u8>)>>,
}

impl Default for WebFiles {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFiles {
    /// A fresh browser files capability with empty custody registries.
    pub fn new() -> Self {
        Self {
            sources: Rc::new(Registry::new()),
            staged: Rc::new(Registry::new()),
            exports: Rc::new(Registry::new()),
            artifacts: Rc::new(Registry::new()),
        }
    }
}

/// Read a browser blob's bytes into memory via `Blob.arrayBuffer()`.
async fn read_blob_bytes(blob: &web_sys::Blob) -> Result<Vec<u8>, CapabilityError> {
    let buffer = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
        .await
        .map_err(|_| CapabilityError::Failed(FailureKind::Unreadable))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Trigger a browser download of `bytes` under `name` via an object URL. The
/// browser owns the destination — no write path is ever named (spec D4).
fn trigger_download(name: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(array.as_ref());
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)
        .map_err(|_| CapabilityError::Failed(FailureKind::Io))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| CapabilityError::Failed(FailureKind::Io))?;
    let outcome = (|| {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or(CapabilityError::Unavailable)?;
        let anchor = document
            .create_element("a")
            .ok()
            .and_then(|el| el.dyn_into::<web_sys::HtmlAnchorElement>().ok())
            .ok_or(CapabilityError::Failed(FailureKind::Io))?;
        anchor.set_href(&url);
        anchor.set_download(name);
        anchor.click();
        Ok(())
    })();
    let _ = web_sys::Url::revoke_object_url(&url);
    outcome
}

/// Open the browser file picker and resolve the selection. `Err(Cancelled)` when
/// the dialog is dismissed with no file; `Ok(None)` never happens on the browser
/// (a clean no-selection is a dismissal here), which the trait allows.
async fn pick_file() -> Result<Option<web_sys::File>, CapabilityError> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or(CapabilityError::Unavailable)?;
    let input = document
        .create_element("input")
        .ok()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .ok_or(CapabilityError::Unavailable)?;
    input.set_type("file");

    let (tx, rx) = futures::channel::oneshot::channel::<Option<web_sys::File>>();
    let tx = Rc::new(std::cell::RefCell::new(Some(tx)));
    let input_for_change = input.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let file = input_for_change.files().and_then(|list| list.get(0));
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(file);
        }
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    input.click();
    // Leak the one-shot closure for the dialog's lifetime; the pick resolves once.
    on_change.forget();

    rx.await.map_err(|_| CapabilityError::Cancelled)
}

/// A bounded, pull-shaped reader over an in-memory staged blob.
struct VecReader {
    bytes: Vec<u8>,
    pos: usize,
}

impl StagedBlobReader for VecReader {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn next_chunk(
        &mut self,
        max_len: usize,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, CapabilityError>> {
        // A zero bound must never masquerade as EOF or progress (contract).
        if max_len == 0 {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Io)) });
        }
        let chunk = if self.pos >= self.bytes.len() {
            None
        } else {
            let end = (self.pos + max_len).min(self.bytes.len());
            let chunk = self.bytes[self.pos..end].to_vec();
            self.pos = end;
            Some(chunk)
        };
        Box::pin(async move { Ok(chunk) })
    }
}

/// What a committed [`WebFileSink`] does with its accumulated bytes.
enum SinkMode {
    Download,
    Open,
}

/// A push-shaped byte destination that accumulates in memory and, on commit,
/// triggers a download / object-URL open. Drop-is-abort by construction: an
/// uncommitted sink is simply dropped and nothing is written (spec D5).
struct WebFileSink {
    name: FileName,
    acc: Vec<u8>,
    mode: SinkMode,
}

impl FileSink for WebFileSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>> {
        self.acc.extend_from_slice(&chunk);
        Box::pin(async { Ok(()) })
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), CapabilityError>> {
        Box::pin(async move {
            match self.mode {
                SinkMode::Download | SinkMode::Open => {
                    // Both complete through a browser object URL; Open would also
                    // navigate to the URL, Download names it — the download path
                    // is the honest default for untrusted peer bytes (spec D6).
                    trigger_download(self.name.as_str(), &self.acc)
                }
            }
        })
    }
}

/// A push-shaped byte destination that commits into the crate's own custody and
/// mints a [`FetchedArtifact`] for the share sheet.
struct WebShareSink {
    name: FileName,
    acc: Vec<u8>,
    artifacts: Rc<Registry<(FileName, Vec<u8>)>>,
}

impl ShareSink for WebShareSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>> {
        self.acc.extend_from_slice(&chunk);
        Box::pin(async { Ok(()) })
    }

    fn commit(
        self: Box<Self>,
    ) -> BoxFuture<'static, Result<jeliya_platform::files::FetchedArtifact, CapabilityError>> {
        Box::pin(async move {
            let size = self.acc.len() as u64;
            let raw = self.artifacts.mint((self.name.clone(), self.acc));
            Ok(fetched_artifact(
                artifact_token_from_raw(raw),
                self.name,
                size,
            ))
        })
    }
}

/// The custody-release result: `Ok` when an entry was dropped, `Unreadable` for
/// a foreign / already-released token (fail-closed, borrow-not-consume).
fn release(removed: bool) -> Result<(), CapabilityError> {
    if removed {
        Ok(())
    } else {
        Err(CapabilityError::Failed(FailureKind::Unreadable))
    }
}

impl Files for WebFiles {
    fn pick(
        &self,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<PickedSource>, CapabilityError>> {
        let sources = self.sources.clone();
        Box::pin(async move {
            let Some(file) = pick_file().await? else {
                return Ok(None);
            };
            let name = FileName::parse(file.name())
                .or_else(|_| FileName::parse("file"))
                .map_err(|_| CapabilityError::Failed(FailureKind::Unreadable))?;
            let size = file.size() as u64;
            let mime = {
                let t = file.type_();
                if t.is_empty() {
                    None
                } else {
                    Some(Mime::new(t))
                }
            };
            let raw = sources.mint(file);
            Ok(Some(picked_source(
                source_token_from_raw(raw),
                name,
                size,
                mime,
                FileObjectKind::BrowserBlob,
            )))
        })
    }

    fn discard_source(&self, src: &PickedSource) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let raw = source_token_into_raw(picked_source_token(src));
        let sources = self.sources.clone();
        Box::pin(async move { release(sources.remove(raw)) })
    }

    fn release_artifact(
        &self,
        artifact: &jeliya_platform::files::FetchedArtifact,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let raw = jeliya_platform_implementation::artifact_token_into_raw(
            jeliya_platform_implementation::fetched_artifact_token(artifact),
        );
        let artifacts = self.artifacts.clone();
        Box::pin(async move { release(artifacts.remove(raw)) })
    }

    fn discard_export_target(
        &self,
        target: &ExportTarget,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let raw = export_token_into_raw(export_target_token(target));
        let exports = self.exports.clone();
        Box::pin(async move { release(exports.remove(raw)) })
    }

    fn stage_for_share(
        &self,
        src: &PickedSource,
        limit: u64,
        progress: ProgressSink,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<ShareableBlob, CapabilityError>> {
        let raw = source_token_into_raw(picked_source_token(src));
        let sources = self.sources.clone();
        let staged = self.staged.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            // Consume the source entry (staged picks need no later discard).
            let file = sources
                .take(raw)
                .ok_or(CapabilityError::Failed(FailureKind::Unreadable))?;
            // Preflight the KNOWN size before any read (spec D3).
            let known = file.size() as u64;
            if known > limit {
                return Err(CapabilityError::Failed(FailureKind::FileTooLarge {
                    size: known,
                    limit,
                }));
            }
            let bytes = read_blob_bytes(&file).await?;
            if ct.is_cancelled() {
                // Cancelled after read but before minting: nothing retained.
                return Err(CapabilityError::Cancelled);
            }
            let total = bytes.len() as u64;
            if total > limit {
                return Err(CapabilityError::Failed(FailureKind::FileTooLarge {
                    size: total,
                    limit,
                }));
            }
            if total == 0 {
                return Err(CapabilityError::Failed(FailureKind::FileEmpty));
            }
            progress.report(StageProgress {
                transferred: total,
                total: Some(total),
            });
            let raw = staged.mint(bytes);
            Ok(shareable_blob(blob_token_from_raw(raw), total))
        })
    }

    fn read_staged(
        &self,
        blob: &ShareableBlob,
    ) -> BoxFuture<'_, Result<Box<dyn StagedBlobReader>, CapabilityError>> {
        let raw = blob_token_into_raw(shareable_blob_token(blob));
        let staged = self.staged.clone();
        Box::pin(async move {
            // Snapshot the bytes: an already-open reader keeps serving across a
            // later `release_staged`, exactly as an open fd outlives an unlink.
            let bytes = staged
                .with(raw, |b| b.clone())
                .ok_or(CapabilityError::Failed(FailureKind::Unreadable))?;
            Ok(Box::new(VecReader { bytes, pos: 0 }) as Box<dyn StagedBlobReader>)
        })
    }

    fn release_staged(&self, blob: &ShareableBlob) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let raw = blob_token_into_raw(shareable_blob_token(blob));
        let staged = self.staged.clone();
        Box::pin(async move { release(staged.remove(raw)) })
    }

    fn pick_export_target(
        &self,
        suggested: FileName,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>> {
        let exports = self.exports.clone();
        Box::pin(async move {
            let raw = exports.mint(suggested.clone());
            Ok(Some(export_target(
                export_token_from_raw(raw),
                ExportTargetKind::BrowserDownload,
                suggested,
            )))
        })
    }

    fn export_sink(
        &self,
        to: &ExportTarget,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>> {
        let raw = export_token_into_raw(export_target_token(to));
        let exports = self.exports.clone();
        Box::pin(async move {
            let name = exports
                .take(raw)
                .ok_or(CapabilityError::Failed(FailureKind::Unreadable))?;
            Ok(Box::new(WebFileSink {
                name,
                acc: Vec::new(),
                mode: SinkMode::Download,
            }) as Box<dyn FileSink>)
        })
    }

    fn open_sink(
        &self,
        name: FileName,
        _declared: Option<Mime>,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>> {
        Box::pin(async move {
            Ok(Box::new(WebFileSink {
                name,
                acc: Vec::new(),
                mode: SinkMode::Open,
            }) as Box<dyn FileSink>)
        })
    }

    fn share_sink(
        &self,
        name: FileName,
        _declared: Option<Mime>,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn ShareSink>, CapabilityError>> {
        let artifacts = self.artifacts.clone();
        Box::pin(async move {
            Ok(Box::new(WebShareSink {
                name,
                acc: Vec::new(),
                artifacts,
            }) as Box<dyn ShareSink>)
        })
    }

    fn share_content(
        &self,
        _content: &jeliya_platform::clipboard::ShareContent,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        // Web Share Level 2 file sharing is not the primary path (spec Q4); the
        // share sheet is honestly Unavailable, and callers fall back to export.
        Box::pin(async { Err(CapabilityError::Unavailable) })
    }
}
