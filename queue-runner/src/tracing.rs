use tracing_subscriber::{Registry, layer::SubscriberExt as _};

#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;

#[cfg(feature = "otel")]
fn resource() -> opentelemetry_sdk::Resource {
    opentelemetry_sdk::Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_schema_url(
            [opentelemetry::KeyValue::new(
                opentelemetry_semantic_conventions::attribute::SERVICE_VERSION,
                env!("CARGO_PKG_VERSION"),
            )],
            opentelemetry_semantic_conventions::SCHEMA_URL,
        )
        .build()
}

pub struct TracingGuard {
    #[cfg(feature = "otel")]
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,

    reload_handle: tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >,
}

impl TracingGuard {
    pub fn change_log_level(&self, new_filter: tracing_subscriber::EnvFilter) {
        let _ = self.reload_handle.modify(|filter| *filter = new_filter);
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        if let Err(err) = self.tracer_provider.shutdown() {
            eprintln!("{err:?}");
        }
    }
}

#[cfg(feature = "otel")]
fn init_tracer_provider() -> anyhow::Result<opentelemetry_sdk::trace::SdkTracerProvider> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;

    Ok(opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(resource())
        .with_batch_exporter(exporter)
        .build())
}

#[cfg(feature = "tokio-console")]
pub fn init() -> anyhow::Result<TracingGuard> {
    tracing_log::LogTracer::init()?;
    let (log_env_filter, reload_handle) = tracing_subscriber::reload::Layer::new(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    let fmt_layer = tracing_subscriber::fmt::layer().compact();
    let console_layer = console_subscriber::spawn();
    let subscriber = Registry::default()
        .with(log_env_filter)
        .with(fmt_layer)
        .with(console_layer);
    #[cfg(feature = "otel")]
    {
        let provider = init_tracer_provider()?;
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        let subscriber = subscriber.with(tracing_opentelemetry::OpenTelemetryLayer::new(tracer));
        tracing::subscriber::set_global_default(subscriber)?;
        Ok(TracingGuard {
            tracer_provider: provider,
            reload_handle,
        })
    }

    #[cfg(not(feature = "otel"))]
    {
        tracing::subscriber::set_global_default(subscriber)?;
        Ok(TracingGuard { reload_handle })
    }
}

#[cfg(not(feature = "tokio-console"))]
pub fn init() -> anyhow::Result<TracingGuard> {
    tracing_log::LogTracer::init()?;
    let (log_env_filter, reload_handle) = tracing_subscriber::reload::Layer::new(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    let fmt_layer = tracing_subscriber::fmt::layer().compact();
    let subscriber = Registry::default().with(log_env_filter).with(fmt_layer);

    #[cfg(feature = "otel")]
    {
        let provider = init_tracer_provider()?;
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        let subscriber = subscriber.with(tracing_opentelemetry::OpenTelemetryLayer::new(tracer));
        tracing::subscriber::set_global_default(subscriber)?;
        Ok(TracingGuard {
            tracer_provider: provider,
            reload_handle,
        })
    }

    #[cfg(not(feature = "otel"))]
    {
        tracing::subscriber::set_global_default(subscriber)?;
        Ok(TracingGuard { reload_handle })
    }
}
