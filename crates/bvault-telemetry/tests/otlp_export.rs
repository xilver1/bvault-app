//! End-to-end check that spans actually leave the process.
//!
//! Everything else about this crate is compile-time-obvious; this is the part
//! that isn't. A stand-in collector — a bare TCP socket speaking just enough
//! HTTP/1.1 — takes the place of Tempo, and the test asserts on the bytes the
//! exporter really put on the wire: the OTLP/HTTP request line, the protobuf
//! content type, and the service name inside the payload.
//!
//! It lives in `tests/` rather than beside the unit tests because [`init`]
//! installs a *global* subscriber: one per process, so this test needs a
//! process of its own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn spans_reach_an_otlp_http_collector() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stand-in collector");
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("exporter never connected");
        let request = read_http_request(&mut stream);
        // The exporter treats a non-2xx as a failure and retries; answer
        // cleanly so the test observes one export, not a retry storm.
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let _ = stream.flush();
        let _ = tx.send(request);
    });

    // The generic endpoint form — the one a ConfigMap will carry. The exporter
    // is responsible for appending `/v1/traces`, which is asserted below.
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", format!("http://{addr}"));
    std::env::set_var("OTEL_TRACES_FILTER", "info");
    let telemetry = bvault_telemetry::init("telemetry-test");

    let traceparent = tracing::info_span!("outer")
        .in_scope(|| tracing::info_span!("inner").in_scope(bvault_telemetry::current_traceparent));

    // Propagation: a live span must serialize to a W3C traceparent, or the
    // queue hop silently degrades to orphan traces.
    let traceparent = traceparent.expect("an active span should yield a traceparent");
    let fields: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(fields.len(), 4, "traceparent shape: {traceparent}");
    assert_eq!(fields[0], "00", "W3C version");
    assert_eq!(fields[1].len(), 32, "trace id");
    assert_eq!(fields[2].len(), 16, "span id");
    assert_eq!(
        fields[3], "01",
        "sampled flag — default sampler is always-on"
    );

    assert!(
        bvault_telemetry::current_traceparent().is_none(),
        "outside any span there is nothing to propagate"
    );

    // The queue hop, exactly as the analysis worker performs it: a fresh span,
    // re-parented onto a stored traceparent, must land in the *same trace* as
    // the request that enqueued the job. If this regresses, Tempo silently
    // shows two disconnected traces instead of one.
    let adopted = tracing::info_span!("claimed-job");
    bvault_telemetry::set_parent(&adopted, Some(traceparent.as_str()));
    let adopted_traceparent = adopted
        .in_scope(bvault_telemetry::current_traceparent)
        .expect("re-parented span should yield a traceparent");
    assert_eq!(
        adopted_traceparent.split('-').nth(1),
        Some(fields[1]),
        "trace id must survive the queue hop: {traceparent} -> {adopted_traceparent}"
    );
    assert_ne!(
        adopted_traceparent.split('-').nth(2),
        Some(fields[2]),
        "the consumer gets its own span id, not the producer's"
    );

    // Shutting the guard down flushes; without it the batch processor would
    // still be waiting out its scheduled delay.
    telemetry.shutdown();

    let request = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("no OTLP export arrived at the collector");
    let head = String::from_utf8_lossy(&request).to_string();

    assert!(
        head.starts_with("POST /v1/traces "),
        "OTEL_EXPORTER_OTLP_ENDPOINT should get the signal path appended; got:\n{}",
        head.lines().next().unwrap_or_default()
    );
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/x-protobuf"),
        "expected OTLP/HTTP protobuf encoding"
    );
    // Resource attributes are protobuf-encoded, but the service name travels as
    // a plain UTF-8 string inside it — enough to prove the resource made it.
    assert!(
        find(&request, b"telemetry-test"),
        "service.name missing from the exported payload"
    );
    assert!(
        find(&request, b"inner") && find(&request, b"outer"),
        "both spans should be in the export"
    );
}

/// Read one HTTP request: headers, then exactly `Content-Length` body bytes.
fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];

    // Headers first — read until the blank line that ends them.
    let header_end = loop {
        if let Some(at) = find_at(&buf, b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return buf,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return buf,
        }
    };

    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);

    while buf.len() < header_end + content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    buf
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    find_at(haystack, needle).is_some()
}

fn find_at(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
