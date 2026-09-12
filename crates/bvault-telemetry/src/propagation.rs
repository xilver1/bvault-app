//! W3C trace-context propagation: how one span in one process becomes the
//! parent of a span in the next.
//!
//! bvault has two hops that need this, and they are not the same shape:
//!
//! * **HTTP** (gateway → yt-dlp-ingest, client → gateway) — context rides in
//!   request headers, via [`extract_context`] and [`current_traceparent`].
//! * **The job queue** (gateway → analysis worker) — the hop is a Postgres row
//!   and a claim that may happen minutes later, so the `traceparent` is stored
//!   on the row and re-attached with [`set_parent`] when the worker picks it up.
//!   That is what makes "user pressed Analyze" and "this track finished
//!   analysing" one trace in Tempo rather than two unrelated ones.
//!
//! Every function here is a no-op when tracing is disabled: with no global
//! propagator installed, injection yields nothing and extraction yields an
//! empty context, so callers need no `if traces_enabled` branches.

use std::collections::HashMap;

use opentelemetry::global;
use opentelemetry::Context;
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// The W3C header name, so call sites don't hand-spell it.
pub const TRACEPARENT: &str = "traceparent";

/// Read an incoming request's trace context. Returns an empty context when the
/// caller sent none — which is then simply the root of a new trace.
pub fn extract_context(headers: &http::HeaderMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

/// Serialize the *current* span's context as a `traceparent` value, for
/// carrying across a hop that isn't an HTTP request — a queue row, a callback
/// URL, a log line someone will correlate by hand.
///
/// `None` means there is nothing to propagate: tracing is off, or this code is
/// not running inside a sampled span.
pub fn current_traceparent() -> Option<String> {
    let context = tracing::Span::current().context();
    let mut carrier: HashMap<String, String> = HashMap::new();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&context, &mut carrier));
    carrier.remove(TRACEPARENT)
}

/// Rebuild a context from a stored `traceparent`. A malformed or unknown value
/// yields an empty context, so a bad header can only cost a link, never a span.
pub fn context_from_traceparent(traceparent: &str) -> Context {
    let carrier = HashMap::from([(TRACEPARENT.to_string(), traceparent.to_string())]);
    global::get_text_map_propagator(|propagator| propagator.extract(&carrier))
}

/// Adopt `traceparent` as `span`'s remote parent. Pass `None` and the span stays
/// a root — exactly what should happen for work enqueued before this change
/// shipped, or enqueued while tracing was switched off.
pub fn set_parent(span: &tracing::Span, traceparent: Option<&str>) {
    if let Some(traceparent) = traceparent {
        // Errors here mean the subscriber has no OTel layer (tracing disabled).
        let _ = span.set_parent(context_from_traceparent(traceparent));
    }
}

/// Stamp the current span's trace context onto outgoing request headers.
///
/// Use this for **internal** calls only. Trace ids are internal topology; there
/// is no reason to hand them to Google Drive or YouTube along with a request.
pub fn inject_into_headers(headers: &mut http::HeaderMap) {
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers))
    });
}
