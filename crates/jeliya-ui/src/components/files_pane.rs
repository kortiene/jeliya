//! The Files room-destination pane (#181 §5.D): the file list with truthful
//! availability, a Share action, and per-file Fetch / Export with progress and
//! cancel. Replaces the #178 skeleton at `/rooms/:roomId/files`.
//!
//! All decisions live in the host-tested [`crate::files`] orchestration; this
//! component only renders truthful state and drives the flows. Booting is
//! *unknown, not zero* (spec D8): before `file.list` answers, the pane shows a
//! loading state, never "no files". A failed list shows the real error + Retry.
//! Availability is read only from the `file.list` fields (spec D2); the declared
//! content type is shown *labeled untrusted* and never used to render bytes
//! inline (spec D6). Over-limit and provider failures render the served-value,
//! redacting copy (spec D3/§6).

use dioxus::prelude::*;
use jeliya_api::{Cursor, Direction, FileId, FileList, Link, LinkReason, Page, RoomId};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::CancelToken;

use crate::files::{
    self, run_fetch_export, run_upload, FileListModel, FileRowView, FlowFailure, ProviderView,
    Settled, SizeLimit,
};
use crate::l10n::{use_formats, use_strings, ErrorDisplay};
use crate::PlatformServices;

/// A first page of a paged read. The `limit` is the page size (not the file
/// -size limit); 100 rows is the initial window, `truncated` carries the rest.
fn first_page() -> Page {
    Page {
        cursor: Cursor::Start,
        direction: Direction::Forward,
        limit: 100,
    }
}

/// The served size ceiling as an orchestration [`files::ServedLimit`] — `None`
/// when the seam has not surfaced it (spec R1); never a compiled-in constant.
fn served_limit(limit: Option<u64>) -> files::ServedLimit {
    limit.map(SizeLimit::new)
}

/// The Files pane.
#[component]
pub fn FilesPane(
    handle: ClientHandle,
    services: PlatformServices,
    room_id: RoomId,
    /// The deep-linked selected file, if any (`RoomDest::Files { item }`).
    #[props(default)]
    selected: Option<FileId>,
    /// The served `max_shared_file_bytes`, when the seam surfaced it (spec D3/R1).
    #[props(default)]
    max_shared_file_bytes: Option<u64>,
    /// Whether the room is a read-only archive (suppress share/fetch — spec D8).
    #[props(default)]
    read_only: bool,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();

    // A reload counter so Retry / a successful share re-reads the list.
    let mut reload = use_signal(|| 0_u32);
    // Transient action state: a friendly error to show, and (while a transfer is
    // in flight) the cancel token that aborts it.
    let mut notice = use_signal(|| None::<String>);
    let mut busy = use_signal(|| None::<CancelToken>);

    let list = {
        let handle = handle.clone();
        let room_id = room_id.clone();
        use_resource(move || {
            let handle = handle.clone();
            let room_id = room_id.clone();
            let _ = reload(); // reactive dependency: bumping `reload` re-reads
            async move {
                match handle
                    .call::<FileList>(
                        FileList {
                            room_id,
                            page: first_page(),
                        },
                        Dedup::None,
                    )
                    .await
                {
                    Ok(out) => Ok(FileListModel::from_out(out)),
                    Err(error) => Err(files::errors::FlowStop::from_call(&error)),
                }
            }
        })
    };

    // The Share action: pick → stage → file.share, cancellable, cleaning up.
    let on_share = {
        let handle = handle.clone();
        let services = services.clone();
        let room_id = room_id.clone();
        move |_| {
            if read_only {
                return;
            }
            let handle = handle.clone();
            let services = services.clone();
            let room_id = room_id.clone();
            let ct = CancelToken::new();
            busy.set(Some(ct.clone()));
            notice.set(None);
            let progress = files::transfer::progress_sink(
                |_p| { /* progress signal wiring is a follow-up */ },
            );
            spawn(async move {
                let outcome = run_upload(
                    &handle,
                    &services,
                    room_id,
                    served_limit(max_shared_file_bytes),
                    progress,
                    &ct,
                )
                .await;
                busy.set(None);
                match outcome {
                    Settled::Ok(_) => {
                        reload.set(reload() + 1);
                    }
                    Settled::Failed(failure) => {
                        notice.set(Some(
                            ErrorDisplay::flow_failure(strings, formats, &failure).message,
                        ));
                    }
                    // A cancel leaves the prior state untouched (spec D5).
                    Settled::Cancelled => {}
                }
            });
        }
    };

    let busy_now = busy().is_some();
    let cancel = move |_| {
        if let Some(ct) = busy() {
            ct.cancel();
        }
    };

    rsx! {
        section { class: "files-pane", id: "files-pane",
            header { class: "pane-header",
                h2 { class: "pane-title", "{strings.files_heading()}" }
                if read_only {
                    p { class: "muted", id: "files-read-only", "{strings.files_read_only()}" }
                } else if busy_now {
                    button { class: "btn btn-sm", id: "files-cancel", onclick: cancel,
                        "{strings.files_cancel_action()}"
                    }
                } else {
                    button { class: "btn btn-primary btn-sm", id: "files-share", onclick: on_share,
                        "{strings.files_share_action()}"
                    }
                }
            }
            if let Some(message) = notice() {
                div { class: "error-note", id: "files-notice", "{message}" }
            }
            {
                match &*list.read_unchecked() {
                    None => rsx! {
                        div { class: "muted", id: "files-loading", "{strings.files_loading()}" }
                    },
                    Some(Err(stop)) => {
                        let failure = match stop {
                            files::errors::FlowStop::Failed(f) => f.clone(),
                            files::errors::FlowStop::Cancelled => FlowFailure::Transport,
                        };
                        let message = ErrorDisplay::flow_failure(strings, formats, &failure).message;
                        rsx! {
                            div { class: "error-note", id: "files-failed",
                                p { "{message}" }
                                button {
                                    class: "btn btn-sm",
                                    id: "files-retry",
                                    onclick: move |_| reload.set(reload() + 1),
                                    "{strings.files_retry_action()}"
                                }
                            }
                        }
                    }
                    Some(Ok(model)) if model.rows.is_empty() => rsx! {
                        div { class: "muted", id: "files-empty", "{strings.files_empty()}" }
                    },
                    Some(Ok(model)) => {
                        let rows = model.rows.clone();
                        rsx! {
                            ul { class: "files-list", id: "files-list",
                                for row in rows {
                                    FileRow {
                                        key: "{row.file_id}",
                                        handle: handle.clone(),
                                        services: services.clone(),
                                        room_id: room_id.clone(),
                                        row: row.clone(),
                                        selected: selected.as_ref() == Some(&row.file_id),
                                        max_shared_file_bytes,
                                        read_only,
                                        notice,
                                        busy,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A localized label for a provider's link evidence (spec D2). The reason for a
/// missing link is shown, so "not currently fetchable" carries *why*.
fn link_label(strings: &dyn crate::l10n::Catalog, link: &Link) -> &'static str {
    match link {
        Link::Direct { .. } => strings.wire_path_direct(),
        Link::Relay { .. } => strings.wire_path_relay(),
        Link::NotConnected { reason } => match reason {
            LinkReason::NeverDialed => strings.status_stopped(),
            LinkReason::DialFailed | LinkReason::NoRoute => strings.status_failed(),
            LinkReason::Closed => strings.status_interrupted(),
        },
    }
}

/// One file row: name, size, digest short-form, declared-type-as-untrusted,
/// provider evidence, and (when fetchable + a valid name) an Export action.
#[component]
fn FileRow(
    handle: ClientHandle,
    services: PlatformServices,
    room_id: RoomId,
    row: FileRowView,
    selected: bool,
    max_shared_file_bytes: Option<u64>,
    read_only: bool,
    notice: Signal<Option<String>>,
    busy: Signal<Option<CancelToken>>,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();

    let name = row
        .name
        .as_ref()
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| strings.files_name_hidden().to_string());
    let size = formats.bytes(row.bytes);
    let preview = files::classify_preview(&row.declared_content_type);

    let availability = if row.self_hosted {
        strings.files_avail_on_device()
    } else if row.can_fetch() {
        strings.files_avail_fetchable()
    } else {
        strings.files_avail_not_fetchable()
    };

    // Export is offered only when the file is fetchable AND its name validated
    // (a hostile name has no safe export target — spec D4) AND the room is live.
    let can_export = row.can_fetch() && row.name.is_some() && !read_only;
    let export_name = row.name.clone();
    let file_id = row.file_id.clone();

    let on_export = move |_| {
        let (Some(name), false) = (export_name.clone(), busy().is_some()) else {
            return;
        };
        let handle = handle.clone();
        let services = services.clone();
        let room_id = room_id.clone();
        let file_id = file_id.clone();
        let signed_bytes = row.bytes;
        let mut notice = notice;
        let mut busy = busy;
        let ct = CancelToken::new();
        busy.set(Some(ct.clone()));
        spawn(async move {
            let outcome = run_fetch_export(
                &handle,
                &services,
                room_id,
                file_id,
                signed_bytes,
                name,
                max_shared_file_bytes.map(SizeLimit::new),
                &ct,
            )
            .await;
            busy.set(None);
            if let Settled::Failed(failure) = outcome {
                notice.set(Some(
                    ErrorDisplay::flow_failure(strings, formats, &failure).message,
                ));
            }
        });
    };

    let class = if selected {
        "file-row selected"
    } else {
        "file-row"
    };

    rsx! {
        li { class: "{class}", id: "file-row-{row.file_id}",
            div { class: "file-main",
                span { class: "file-name", "{name}" }
                span { class: "file-size mono", "{size}" }
            }
            div { class: "file-meta muted",
                span { class: "file-availability", "{availability}" }
                span { class: "file-declared-type",
                    "{strings.files_declared_type_label()}: {row.declared_content_type}"
                }
                span { class: "file-digest mono", "{strings.files_digest_label()}: {row.digest_short}" }
            }
            if !row.providers.is_empty() {
                ul { class: "file-providers",
                    for provider in row.providers.iter() {
                        ProviderRow { key: "{provider.device_id}", provider: provider.clone() }
                    }
                }
            }
            div { class: "file-actions",
                if can_export {
                    button { class: "btn btn-sm", id: "file-export-{row.file_id}", onclick: on_export,
                        "{strings.files_export_action()}"
                    }
                }
                // The opt-in preview affordance appears only for inert allowlisted
                // types (spec D6); it never auto-renders peer bytes.
                if preview.offers_preview() && can_export {
                    span { class: "file-preview-note muted", "{strings.files_preview_action()}" }
                }
            }
        }
    }
}

/// One provider's link evidence (spec D2).
#[component]
fn ProviderRow(provider: ProviderView) -> Element {
    let strings = use_strings();
    let label = link_label(strings, &provider.link);
    rsx! {
        li { class: "file-provider",
            span { class: "provider-device mono", "{provider.device_id}" }
            span { class: "provider-link", "{label}" }
        }
    }
}
