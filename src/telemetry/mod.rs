use std::{collections::HashMap, env, sync::OnceLock, time::Duration};

use anyhow::Result;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram, UpDownCounter},
    propagation::Extractor,
    trace::{TraceContextExt, TracerProvider},
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter};
use opentelemetry_prometheus_text_exporter::PrometheusExporter;
use opentelemetry_sdk::{
    Resource,
    logs::{SdkLoggerProvider, log_processor_with_async_runtime::BatchLogProcessor},
    metrics::SdkMeterProvider,
    propagation::TraceContextPropagator,
    runtime,
    trace::{SdkTracerProvider, span_processor_with_async_runtime::BatchSpanProcessor},
};
use serde_json::{Value, json};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::application::job::ClaimedJob;
use crate::application::job::JobTracer;
use crate::application::user::TraceContextProvider;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    meter_provider: SdkMeterProvider,
}

static PROMETHEUS_EXPORTER: OnceLock<PrometheusExporter> = OnceLock::new();
static METRICS: OnceLock<Metrics> = OnceLock::new();

struct Metrics {
    http_requests: Counter<u64>,
    http_duration: Histogram<f64>,
    http_active: UpDownCounter<i64>,
    rate_limit_rejections: Counter<u64>,
    job_outcomes: Counter<u64>,
    job_duration: Histogram<f64>,
    cleanup_count: Counter<u64>,
    worker_errors: Counter<u64>,
}

impl TelemetryGuard {
    pub fn shutdown(self) {
        if let Err(error) = self.meter_provider.shutdown() {
            eprintln!("failed to flush OpenTelemetry metrics: {error}");
        }
        if let Some(provider) = self.logger_provider
            && let Err(error) = provider.shutdown()
        {
            eprintln!("failed to flush OpenTelemetry logs: {error}");
        }
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

    let service_name =
        env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| env!("CARGO_PKG_NAME").to_owned());
    let resource = Resource::builder().with_service_name(service_name).build();
    let prometheus_enabled =
        env::var("METRICS_PROMETHEUS_BEARER_TOKEN").is_ok_and(|value| !value.trim().is_empty());
    let mut meter_builder = SdkMeterProvider::builder().with_resource(resource.clone());
    if prometheus_enabled {
        let exporter = PrometheusExporter::new();
        meter_builder = meter_builder.with_reader(exporter.clone());
        let _ = PROMETHEUS_EXPORTER.set(exporter);
    }
    if endpoint.is_some() {
        let exporter = MetricExporter::builder().with_http().build()?;
        meter_builder = meter_builder.with_periodic_exporter(exporter);
    }
    let meter_provider = meter_builder.build();
    global::set_meter_provider(meter_provider.clone());
    initialize_metrics();

    let (tracer_provider, logger_provider) = if endpoint.is_some() {
        // Let the OTLP exporter read the generic endpoint from the environment.
        // It appends `/v1/traces` for `OTEL_EXPORTER_OTLP_ENDPOINT`, whereas a
        // programmatic endpoint is treated as an already-complete signal URL.
        let exporter = SpanExporter::builder().with_http().build()?;
        let provider = SdkTracerProvider::builder()
            .with_span_processor(BatchSpanProcessor::builder(exporter, runtime::Tokio).build())
            .with_resource(resource.clone())
            .build();
        let log_exporter = LogExporter::builder().with_http().build()?;
        let logger_provider = SdkLoggerProvider::builder()
            .with_log_processor(BatchLogProcessor::builder(log_exporter, runtime::Tokio).build())
            .with_resource(resource)
            .build();
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        global::set_tracer_provider(provider.clone());
        tracing_subscriber::registry()
            .with(filter)
            .with(logs)
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(
                OpenTelemetryTracingBridge::new(&logger_provider).with_filter(
                    tracing_subscriber::filter::filter_fn(|metadata| {
                        !metadata.target().starts_with("opentelemetry")
                    }),
                ),
            )
            .init();
        tracing::info!("OpenTelemetry trace and log export enabled");
        (Some(provider), Some(logger_provider))
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(logs)
            .init();
        tracing::info!(
            "OpenTelemetry trace and log export disabled; set OTEL_EXPORTER_OTLP_ENDPOINT to enable it"
        );
        (None, None)
    };

    Ok(TelemetryGuard {
        tracer_provider,
        logger_provider,
        meter_provider,
    })
}

fn initialize_metrics() {
    let meter = global::meter(env!("CARGO_PKG_NAME"));
    let _ = METRICS.set(Metrics {
        http_requests: meter.u64_counter("http.server.requests").build(),
        http_duration: meter
            .f64_histogram("http.server.request.duration")
            .with_unit("s")
            .build(),
        http_active: meter
            .i64_up_down_counter("http.server.active_requests")
            .build(),
        rate_limit_rejections: meter
            .u64_counter("http.server.rate_limit.rejections")
            .build(),
        job_outcomes: meter.u64_counter("job.process.outcomes").build(),
        job_duration: meter
            .f64_histogram("job.process.duration")
            .with_unit("s")
            .build(),
        cleanup_count: meter.u64_counter("job.cleanup.deleted").build(),
        worker_errors: meter.u64_counter("job.worker.errors").build(),
    });
}

pub fn prometheus_text() -> Option<Result<String, std::io::Error>> {
    PROMETHEUS_EXPORTER.get().map(|exporter| {
        let mut bytes = Vec::new();
        exporter.export(&mut bytes)?;
        String::from_utf8(bytes).map_err(std::io::Error::other)
    })
}

pub fn http_request_started(method: &str, route: &str) {
    if let Some(metrics) = METRICS.get() {
        metrics.http_active.add(
            1,
            &[
                KeyValue::new("http.request.method", method.to_owned()),
                KeyValue::new("http.route", route.to_owned()),
            ],
        );
    }
}

pub fn http_request_finished(method: &str, route: &str, status: u16, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let attributes = [
            KeyValue::new("http.request.method", method.to_owned()),
            KeyValue::new("http.route", route.to_owned()),
            KeyValue::new("http.response.status_code", i64::from(status)),
        ];
        metrics.http_active.add(-1, &attributes[..2]);
        metrics.http_requests.add(1, &attributes);
        metrics
            .http_duration
            .record(duration.as_secs_f64(), &attributes);
    }
}

pub fn record_rate_limit_rejection() {
    if let Some(metrics) = METRICS.get() {
        metrics.rate_limit_rejections.add(1, &[]);
    }
}

pub fn record_job_outcome(job_type: &str, outcome: &str, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let attributes = [
            KeyValue::new("job.type", job_type.to_owned()),
            KeyValue::new("job.outcome", outcome.to_owned()),
        ];
        metrics.job_outcomes.add(1, &attributes);
        metrics
            .job_duration
            .record(duration.as_secs_f64(), &attributes);
    }
}

pub fn record_cleanup_count(count: u64) {
    if let Some(metrics) = METRICS.get() {
        metrics.cleanup_count.add(count, &[]);
    }
}

pub fn record_worker_error(operation: &'static str) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .worker_errors
            .add(1, &[KeyValue::new("operation", operation)]);
    }
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
        http.response.status_code = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        otel.status_description = tracing::field::Empty,
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

pub struct OpenTelemetryTraceContext;

impl TraceContextProvider for OpenTelemetryTraceContext {
    fn current(&self) -> Value {
        current_trace_context()
    }
}

pub struct OpenTelemetryJobTracer;

impl JobTracer for OpenTelemetryJobTracer {
    fn span(&self, job: &ClaimedJob) -> Span {
        job_span(job)
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider;

    use super::*;

    #[test]
    fn prometheus_exporter_emits_text_format() {
        let exporter = PrometheusExporter::new();
        let provider = SdkMeterProvider::builder()
            .with_reader(exporter.clone())
            .build();
        let counter = provider
            .meter("test")
            .u64_counter("http.server.requests")
            .build();
        counter.add(1, &[KeyValue::new("http.route", "/api/v1/users")]);
        let mut output = Vec::new();
        exporter.export(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("# TYPE http_server_requests"));
        assert!(output.contains("http_route=\"/api/v1/users\""));
        assert!(!output.contains("client.ip"));
    }
}
