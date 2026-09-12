//! Server spans for the axum services.
//!
//! Drop-in pieces for `tower_http`'s `TraceLayer`, so the gateway and the
//! export-builder describe their HTTP surface identically:
//!
//! ```ignore
//! use tower_http::trace::TraceLayer;
//!
//! Router::new()
//!     // ... routes ...
//!     .layer(
//!         TraceLayer::new_for_http()
//!             .make_span_with(bvault_telemetry::http::OtelMakeSpan)
//!             .on_response(bvault_telemetry::http::OtelOnResponse),
//!     )
//! ```
//!
//! Span names follow the OTel HTTP convention — `GET /playlists/{id}`, built
//! from axum's *matched route* rather than the concrete URI. That distinction is
//! the whole game for a trace store: `/playlists/{id}` is one Tempo series,
//! while `/playlists/9f3c…` would be one series per playlist.

use std::time::Duration;

use axum::extract::MatchedPath;
use http::{Request, Response};
use tower_http::trace::{MakeSpan, OnResponse};
use tracing::{field::Empty, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Opens one server span per request, parented to the caller's trace when the
/// request carries a `traceparent`.
#[derive(Clone, Copy, Debug)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let method = request.method().as_str();
        // Present because the layer sits outside routing; the raw path is only
        // a fallback for requests that matched nothing (404s).
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str)
            .unwrap_or_else(|| request.uri().path());

        // `otel.*` are tracing-opentelemetry's reserved fields: they set the
        // exported span's name, kind and status rather than becoming attributes.
        let span = tracing::info_span!(
            "http-request",
            otel.name = format!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = method,
            http.route = route,
            url.path = request.uri().path(),
            http.response.status_code = Empty,
        );
        // `Err` only when no OTel layer is installed — then there is no parent
        // to adopt and the plain tracing span is already correct.
        let _ = span.set_parent(crate::extract_context(request.headers()));
        span
    }
}

/// Closes the span out with the response status.
#[derive(Clone, Copy, Debug)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        let status = response.status();
        span.record("http.response.status_code", status.as_u16());
        // Only 5xx is the *server's* error. A 401 or a 404 is the span working
        // as intended, and marking those red would make every error-filtered
        // Tempo search useless.
        if status.is_server_error() {
            span.record("otel.status_code", "ERROR");
        }
        tracing::debug!(
            status = status.as_u16(),
            latency_ms = latency.as_millis() as u64,
            "request completed"
        );
    }
}
