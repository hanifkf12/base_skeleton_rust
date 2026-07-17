use std::{collections::HashMap, env};

use anyhow::Result;
use opentelemetry::{
    global,
    propagation::Extractor,
    trace::{TraceContextExt, TracerProvider},
};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator, trace::SdkTracerProvider};
use serde_json::{Value, json};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::application::job::ClaimedJob;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    pub fn shutdown(self) {
        if let Some(provider) = self.tracer_provider
            && let Err(error) = provider.shutdown()
        {
            eprintln!("failed to flush OpenTelemetry traces: {error}");
        }
    }
}

pub fn init() -> Result<TelemetryGuard> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("base_skeleton_rust=debug,tower_http=debug"));
    let logs = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(true);

    let endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let tracer_provider = if let Some(endpoint) = endpoint {
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()?;
        let service_name =
            env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| env!("CARGO_PKG_NAME").to_owned());
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(Resource::builder().with_service_name(service_name).build())
            .build();
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        global::set_tracer_provider(provider.clone());
        tracing_subscriber::registry()
            .with(filter)
            .with(logs)
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .init();
        tracing::info!("OpenTelemetry trace export enabled");
        Some(provider)
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(logs)
            .init();
        tracing::info!(
            "OpenTelemetry trace export disabled; set OTEL_EXPORTER_OTLP_ENDPOINT to enable it"
        );
        None
    };

    Ok(TelemetryGuard { tracer_provider })
}

pub fn current_trace_context() -> Value {
    let context = Span::current().context();
    let mut carrier = HashMap::new();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&context, &mut carrier));
    json!(carrier)
}

pub fn job_span(job: &ClaimedJob) -> Span {
    let carrier = trace_carrier(&job.trace_context);
    let parent = global::get_text_map_propagator(|propagator| propagator.extract(&carrier));
    let span = tracing::info_span!(
        "job.process",
        otel.kind = "consumer",
        job.id = %job.id,
        job.type = %job.job_type,
        job.attempt = job.attempts,
        trace_id = tracing::field::Empty,
        span_id = tracing::field::Empty,
    );
    let _ = span.set_parent(parent);
    record_trace_ids(&span);
    span
}

pub fn http_span<B>(request: &axum::http::Request<B>) -> Span {
    let carrier = HeaderCarrier(request.headers());
    let parent = global::get_text_map_propagator(|propagator| propagator.extract(&carrier));
    let span = tracing::info_span!(
        "http.request",
        otel.name = %format!("{} {}", request.method(), request.uri().path()),
        otel.kind = "server",
        http.request.method = %request.method(),
        url.path = %request.uri().path(),
        trace_id = tracing::field::Empty,
        span_id = tracing::field::Empty,
    );
    let _ = span.set_parent(parent);
    record_trace_ids(&span);
    span
}

fn trace_carrier(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
        .collect()
}

fn record_trace_ids(span: &Span) {
    let span_context = span.context().span().span_context().clone();
    if span_context.is_valid() {
        span.record("trace_id", span_context.trace_id().to_string());
        span.record("span_id", span_context.span_id().to_string());
    }
}

struct HeaderCarrier<'a>(&'a axum::http::HeaderMap);

impl Extractor for HeaderCarrier<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}
