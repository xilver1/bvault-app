//! bvault-telemetry: the one place every service turns on logging and traces.
//!
//! The cluster already ships two of the three observability legs out to the
//! observability host — metrics (Prometheus agent → remote write) and logs
//! (Alloy → Loki). This crate adds the third: OpenTelemetry spans exported over
//! OTLP to Grafana Tempo.
//!
//! Two properties matter more than features here:
//!
//! * **Off by default.** With no `OTEL_EXPORTER_OTLP_ENDPOINT` in the
//!   environment, [`init`] installs exactly the `tracing_subscriber::fmt`
//!   subscriber the services used before — no exporter, no background thread,
//!   no behaviour change for local runs, tests, or the CLI.
//! * **Never fatal.** A misconfigured or unreachable collector degrades to
//!   "logs only" with a warning. An observability outage must not take the
//!   pipeline down with it.
//!
//! Configuration is the standard OTel environment (`OTEL_EXPORTER_OTLP_*`,
//! `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_TRACES_SAMPLER`,
//! `OTEL_SDK_DISABLED`), so it is the same knob set any OTel-aware operator
//! already knows — nothing bvault-specific to learn.

mod propagation;

#[cfg(feature = "http-server")]
pub mod http;

pub use propagation::{
    context_from_traceparent, current_traceparent, extract_context, inject_into_headers,
    set_parent, TRACEPARENT,
};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Live telemetry. Hold it for the lifetime of the process: dropping it flushes
/// whatever the batch processor is still holding, so spans from the last few
/// seconds before shutdown reach Tempo instead of dying with the pod.
#[must_use = "dropping the guard immediately shuts tracing back down"]
pub struct Telemetry {
    provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    /// Flush and stop exporting. Equivalent to dropping the guard, but explicit
    /// where a service wants shutdown ordered against something else.
    pub fn shutdown(mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        if let Some(provider) = self.provider.take() {
            if let Err(e) = provider.shutdown() {
                // Past the point of usefully reporting anywhere else.
                eprintln!("telemetry: exporter shutdown failed: {e}");
            }
        }
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Install the global subscriber, and the OTLP span exporter if one is
/// configured. Call once, first thing in `main`.
///
/// `service_name` is the fallback identity (`service.name`) this process
/// reports to Tempo; `OTEL_SERVICE_NAME` overrides it when set, which is how a
/// second deployment of the same binary distinguishes itself.
pub fn init(service_name: &str) -> Telemetry {
    // The fmt layer keeps its historical filter exactly: RUST_LOG, nothing else.
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env());

    let Some(exporter) = build_exporter() else {
        tracing_subscriber::registry().with(fmt_layer).init();
        return Telemetry { provider: None };
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(sampler_from_env())
        .with_resource(resource(service_name))
        .build();

    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("bvault"))
        // Deliberately *not* RUST_LOG. Trace export must not hinge on a log
        // level: an operator quieting the logs would otherwise silently blind
        // Tempo. `OTEL_TRACES_FILTER` tunes span capture on its own axis.
        .with_filter(EnvFilter::new(env_or(
            "OTEL_TRACES_FILTER",
            "info,sqlx=warn,hyper=warn,h2=warn,tower=warn,reqwest=warn",
        )));

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    // W3C `traceparent` — what Tempo, Grafana, and every other OTel SDK speak.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    opentelemetry::global::set_tracer_provider(provider.clone());

    tracing::info!(service = service_name, "otlp trace export enabled");
    Telemetry {
        provider: Some(provider),
    }
}

/// Build the OTLP exporter, or `None` when tracing is switched off.
///
/// The endpoint is read by the exporter itself rather than passed in, so the
/// spec's two forms behave the way an operator expects:
/// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is used verbatim, while the generic
/// `OTEL_EXPORTER_OTLP_ENDPOINT` gets `/v1/traces` appended.
fn build_exporter() -> Option<SpanExporter> {
    if env_or("OTEL_SDK_DISABLED", "false").eq_ignore_ascii_case("true") {
        return None;
    }
    endpoint_configured()?;

    match SpanExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => Some(exporter),
        Err(e) => {
            // Logs still work; the service still serves. Don't take the pod out
            // over a telemetry misconfiguration.
            eprintln!("telemetry: OTLP exporter unavailable, continuing without traces: {e}");
            None
        }
    }
}

fn endpoint_configured() -> Option<String> {
    [
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
    ]
    .iter()
    .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
}

/// `service.name` plus whatever `OTEL_RESOURCE_ATTRIBUTES` carries (the default
/// builder runs the env detector). An explicit `OTEL_SERVICE_NAME` wins, so the
/// argument is only the fallback.
fn resource(service_name: &str) -> Resource {
    let builder = Resource::builder();
    match std::env::var("OTEL_SERVICE_NAME") {
        Ok(name) if !name.trim().is_empty() => builder.build(),
        _ => builder.with_service_name(service_name.to_string()).build(),
    }
}

/// The subset of `OTEL_TRACES_SAMPLER` worth supporting for a single-household
/// cluster. Default is parent-based always-on: sample everything, but always
/// honour an upstream decision so a trace is never half-recorded.
fn sampler_from_env() -> Sampler {
    let ratio = || {
        env_or("OTEL_TRACES_SAMPLER_ARG", "1.0")
            .parse::<f64>()
            .unwrap_or(1.0)
    };
    match env_or("OTEL_TRACES_SAMPLER", "parentbased_always_on").as_str() {
        "always_on" => Sampler::AlwaysOn,
        "always_off" => Sampler::AlwaysOff,
        "traceidratio" => Sampler::TraceIdRatioBased(ratio()),
        "parentbased_always_off" => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
        "parentbased_traceidratio" => {
            Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio())))
        }
        _ => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These read process-wide environment, so they run as one test rather than
    // racing each other across the harness's threads.
    #[test]
    fn sampler_and_endpoint_read_the_standard_env() {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");
        assert!(endpoint_configured().is_none(), "off without an endpoint");

        // An empty value is "unset", not "export to nowhere" — ConfigMaps make
        // empty strings easy to produce by accident.
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "");
        assert!(endpoint_configured().is_none());

        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://obs:4318");
        assert_eq!(endpoint_configured().as_deref(), Some("http://obs:4318"));

        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "0.25");
        assert!(matches!(
            sampler_from_env(),
            Sampler::TraceIdRatioBased(r) if (r - 0.25).abs() < f64::EPSILON
        ));

        // An unknown sampler name falls back rather than failing to start.
        std::env::set_var("OTEL_TRACES_SAMPLER", "nonsense");
        assert!(matches!(sampler_from_env(), Sampler::ParentBased(_)));

        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }
}
