//! The *outgoing* half of every service-graph edge.
//!
//! Tempo's metrics-generator builds its service graph by pairing two spans: an
//! outgoing span in the caller with the incoming span it parents in the callee.
//! It only ever looks at four span kinds, in two pairings:
//!
//! | Edge | Outgoing | Incoming |
//! |------|----------|----------|
//! | Direct request | `client` | `server` |
//! | Messaging | `producer` | `consumer` |
//!
//! The server and consumer halves already exist ([`crate::http::OtelMakeSpan`]
//! and the analysis worker's job span). This module supplies the other half —
//! without it the graph has nodes but no edges, and questions like "is the
//! gateway's call to yt-dlp-ingest failing?" have no metric behind them.
//!
//! The pairing is by parent-child relationship, so the outgoing span must be
//! *current* when the trace context is propagated — the header injected, or the
//! `traceparent` written to the job row. Both helpers here are designed to be
//! `.instrument()`-ed around exactly that work.

use tracing::{field::Empty, Span};

/// The outgoing half of a messaging edge: work handed to a queue for someone
/// else to pick up later.
///
/// `system` is the messaging backend (`postgresql` here — the jobs table *is*
/// the broker) and `destination` the queue being published to. Keep
/// `destination` low-cardinality: it becomes a service-graph label, so a job
/// kind is right and a job id would be ruinous.
pub fn producer_span(system: &str, destination: &str) -> Span {
    tracing::info_span!(
        "queue-publish",
        otel.name = format!("publish {destination}"),
        otel.kind = "producer",
        otel.status_code = Empty,
        messaging.system = system,
        messaging.operation.name = "publish",
        messaging.destination.name = destination,
    )
}

/// The outgoing half of a direct-request edge: one internal service calling
/// another over HTTP.
///
/// `route` must be the template, not the concrete URL — the same low-cardinality
/// rule the server spans follow. `server_address` names the callee and becomes
/// the far node of the edge.
pub fn client_span(method: &str, server_address: &str, route: &str) -> Span {
    tracing::info_span!(
        "http-client-request",
        otel.name = format!("{method} {route}"),
        otel.kind = "client",
        otel.status_code = Empty,
        http.request.method = method,
        http.route = route,
        server.address = server_address,
        http.response.status_code = Empty,
    )
}

/// Close a [`client_span`] out with what actually happened. `None` means the
/// request never got a response at all (connection refused, timeout).
///
/// Note the asymmetry with server spans, which mark only 5xx as an error: on
/// the *calling* side a 4xx is equally a failure, because the caller asked for
/// something and did not get it. Tempo exposes this as the `status_code`
/// dimension on `traces_spanmetrics_*`, so getting it right is what makes an
/// error rate per dependency mean anything.
pub fn record_client_outcome(span: &Span, status: Option<u16>) {
    match status {
        Some(status) => {
            span.record("http.response.status_code", status);
            if status >= 400 {
                span.record("otel.status_code", "ERROR");
            }
        }
        None => {
            span.record("otel.status_code", "ERROR");
        }
    }
}
