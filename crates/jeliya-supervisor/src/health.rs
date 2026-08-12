//! The unauthenticated `/api/health` probe.
//!
//! A raw loopback HTTP/1.1 GET (no hyper needed — the supervisor stays a thin
//! discovery client, exactly as the daemon's own `health_check` does). The
//! response binds identity: the answering process must be the portfile's PID,
//! because v2 health deliberately **removed `data_dir`** (an unauthenticated
//! endpoint must not leak an absolute path), so PID-on-the-advertised-port is
//! the binding, not a health `data_dir` field (spec §4 D2).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::generation::SeenGeneration;

/// The most of a health response we will ever buffer. The real `/api/health`
/// body is a few hundred bytes; this ceiling is generous for headers + JSON yet
/// small enough that a hostile or misdirected listener on a recycled port
/// cannot make a probe allocate unbounded memory. The read timeout bounds
/// *time*, not *bytes*, so without this a fast local listener could stream
/// hundreds of MB before the deadline and OOM the supervising app (the P2 the
/// review names). A response that would exceed the cap is rejected as "not a
/// health response".
const MAX_HEALTH_RESPONSE_BYTES: u64 = 64 * 1024;

/// The fields the supervisor reads from `/api/health`. Unknown fields (port,
/// version, min_protocol, limits) are ignored. `ok` and `pid` are required for
/// a response to count as a live daemon at all; the generation axes are
/// `Option` so their absence reaches the generation gate as a mismatch rather
/// than being silently defaulted.
#[derive(Debug, Deserialize)]
pub(crate) struct HealthReport {
    /// The daemon's liveness flag.
    pub ok: bool,
    /// The answering process's PID.
    pub pid: u64,
    /// The served protocol generation, or `None` if absent.
    #[serde(default)]
    pub protocol: Option<u64>,
    /// The served storage generation, or `None` if absent.
    #[serde(default)]
    pub storage_generation: Option<u64>,
}

impl HealthReport {
    /// The generation this response advertises.
    pub(crate) fn advertised_generation(&self) -> SeenGeneration {
        SeenGeneration {
            protocol: self.protocol,
            storage_generation: self.storage_generation,
        }
    }

    /// Whether this response proves the process at `expect_pid` is the live
    /// daemon on the probed port: `200 OK`, `ok: true`, and a matching PID. A
    /// recycled port answered by an unrelated listener fails the PID match; a
    /// recycled PID whose process is not a jeliyad on this port fails the probe.
    pub(crate) fn proves_pid(&self, expect_pid: u32) -> bool {
        self.ok && self.pid == u64::from(expect_pid)
    }
}

/// `GET /api/health` on `127.0.0.1:port`, bounded by `connect_timeout` and
/// `read_timeout`. Returns the parsed report, or `None` for any failure (no
/// listener, non-200, non-JSON, missing `ok`/`pid`) — a failure is always
/// "not a healthy daemon here", never a panic.
pub(crate) async fn probe_health(
    port: u16,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Option<HealthReport> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match tokio::time::timeout(connect_timeout, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => stream,
        _ => return None,
    };
    // `Host: 127.0.0.1:<port>` satisfies the daemon's loopback Host gate;
    // `Connection: close` lets us read to EOF.
    let request =
        format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return None;
    }
    // Read through a fixed byte ceiling: `take` caps the bytes buffered
    // regardless of how much the peer sends, so a fast/hostile listener cannot
    // make this allocate without bound before the read timeout fires. If the
    // peer sends exactly the cap we still parse it — an over-cap body simply
    // fails to parse as the small health JSON and is rejected as "not healthy".
    let mut response = Vec::new();
    let mut limited = (&mut stream).take(MAX_HEALTH_RESPONSE_BYTES);
    match tokio::time::timeout(read_timeout, limited.read_to_end(&mut response)).await {
        Ok(Ok(_)) => {}
        _ => return None,
    }
    parse_health_response(&response)
}

/// Split a raw HTTP/1.x response, require a `200` status, and parse the JSON
/// body into a [`HealthReport`]. Isolated from the socket so it is unit-testable
/// without a daemon.
fn parse_health_response(response: &[u8]) -> Option<HealthReport> {
    let text = String::from_utf8_lossy(response);
    let (head, body) = text.split_once("\r\n\r\n")?;
    // Parse the status-line TOKENS, not a prefix: `head.starts_with("HTTP/1.1 200")`
    // also accepts a malformed/extension status like `HTTP/1.1 2000 Not Healthy`,
    // which would let a non-200 listener on a recycled port pass as a live daemon
    // (and satisfy the pre-eviction ownership proof). Require the status token to
    // equal EXACTLY `200`.
    let mut status_line = head.lines().next()?.split_whitespace();
    let version = status_line.next()?;
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return None;
    }
    if status_line.next()? != "200" {
        return None;
    }
    serde_json::from_str::<HealthReport>(body.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_200_health_body_and_binds_pid() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n\
            {\"ok\":true,\"pid\":99,\"port\":7420,\"protocol\":2,\
            \"min_protocol\":2,\"storage_generation\":2}";
        let report = parse_health_response(raw).expect("parses");
        assert!(report.proves_pid(99));
        assert!(
            !report.proves_pid(100),
            "a different pid must not be proven"
        );
        assert_eq!(report.advertised_generation().protocol, Some(2));
        assert_eq!(report.advertised_generation().storage_generation, Some(2));
    }

    /// The status TOKEN must equal exactly `200`: a prefix check accepted an
    /// extension/malformed status like `HTTP/1.1 2000 Not Healthy` with an
    /// otherwise-valid body, letting a non-200 listener on a recycled port pass as
    /// a live daemon (and satisfy the pre-eviction ownership proof).
    #[test]
    fn a_non_200_status_with_a_valid_body_is_rejected() {
        let body = "{\"ok\":true,\"pid\":99,\"port\":7420,\"protocol\":2,\
            \"min_protocol\":2,\"storage_generation\":2}";
        for status in [
            "2000 Not Healthy",
            "500 Internal Server Error",
            "204 No Content",
        ] {
            let raw = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\r\n{body}");
            assert!(
                parse_health_response(raw.as_bytes()).is_none(),
                "status `{status}` must not parse as a healthy 200"
            );
        }
        // The exact-200 path still parses.
        let ok = format!("HTTP/1.1 200 OK\r\n\r\n{body}");
        assert!(parse_health_response(ok.as_bytes()).is_some());
    }

    #[test]
    fn a_non_200_status_is_not_a_healthy_daemon() {
        let raw = b"HTTP/1.1 404 Not Found\r\n\r\n{\"ok\":true,\"pid\":1}";
        assert!(parse_health_response(raw).is_none());
    }

    #[test]
    fn ok_false_never_proves_a_pid() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":false,\"pid\":1}";
        let report = parse_health_response(raw).expect("parses");
        assert!(!report.proves_pid(1), "ok:false is not a live daemon");
    }

    #[test]
    fn a_v1_health_without_storage_generation_reads_absence_not_a_default() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":true,\"pid\":1,\"protocol\":1}";
        let report = parse_health_response(raw).expect("parses");
        assert_eq!(report.advertised_generation().protocol, Some(1));
        assert_eq!(report.advertised_generation().storage_generation, None);
    }

    #[test]
    fn health_without_any_generation_fields_has_both_axes_absent() {
        // A bare `{"ok":true,"pid":1}` must parse, but both generation axes are None.
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":true,\"pid\":1}";
        let report = parse_health_response(raw).expect("parses");
        assert_eq!(report.advertised_generation().protocol, None);
        assert_eq!(report.advertised_generation().storage_generation, None);
    }

    #[test]
    fn non_json_body_is_not_a_health_report() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\nnot json at all";
        assert!(parse_health_response(raw).is_none());
    }

    #[test]
    fn body_missing_pid_field_is_not_a_health_report() {
        // `pid` is required; serde must reject a response that omits it.
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":true}";
        assert!(
            parse_health_response(raw).is_none(),
            "a response without `pid` must not be treated as a live daemon"
        );
    }

    #[test]
    fn body_missing_ok_field_is_not_a_health_report() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"pid\":1}";
        assert!(
            parse_health_response(raw).is_none(),
            "a response without `ok` must not be treated as a live daemon"
        );
    }

    #[test]
    fn http10_200_is_accepted() {
        // Some versions of the daemon may emit HTTP/1.0 responses.
        let raw = b"HTTP/1.0 200 OK\r\n\r\n{\"ok\":true,\"pid\":5}";
        let report = parse_health_response(raw).expect("HTTP/1.0 200 accepted");
        assert!(report.proves_pid(5));
    }

    #[test]
    fn empty_response_is_none() {
        assert!(parse_health_response(b"").is_none());
    }

    #[test]
    fn response_without_blank_line_separator_is_none() {
        // Missing \r\n\r\n means we cannot find the body.
        let raw = b"HTTP/1.1 200 OK{\"ok\":true,\"pid\":1}";
        assert!(parse_health_response(raw).is_none());
    }

    /// A listener whose 200 body places the only valid JSON PAST the byte cap is
    /// rejected: the bounded read stops at the cap (all padding), so parsing the
    /// truncated prefix yields no report. This is the red-before/green-after
    /// witness for the cap — remove the `take(MAX_HEALTH_RESPONSE_BYTES)` and
    /// `read_to_end` consumes the whole body, the trailing JSON parses, and the
    /// probe wrongly reports a healthy pid 7 (and, at scale, buffers unbounded).
    ///
    /// A manually-built current-thread runtime (matching the `validate` unit
    /// tests) keeps this off `#[tokio::test]`, so it does not depend on the
    /// dev-only `macros` feature.
    #[test]
    fn an_oversized_health_body_is_bounded_and_rejected() {
        use tokio::io::AsyncWriteExt as _;
        use tokio::net::TcpListener;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();

        rt.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();

            let server = tokio::spawn(async move {
                if let Ok((mut socket, _)) = listener.accept().await {
                    // Header, then > cap bytes of ASCII-space padding, then the
                    // only valid health JSON — which sits beyond the cap and
                    // must never be reached.
                    let mut body = vec![b' '; (MAX_HEALTH_RESPONSE_BYTES as usize) + 4096];
                    body.extend_from_slice(br#"{"ok":true,"pid":7}"#);
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                }
            });

            let report = probe_health(
                port,
                Duration::from_millis(500),
                // A generous read window so the failure is the CAP, not a timeout.
                Duration::from_secs(2),
            )
            .await;
            assert!(
                report.is_none(),
                "an over-cap body must be rejected: the valid JSON past the cap must never be reached"
            );
            let _ = server.await;
        });
    }
}
