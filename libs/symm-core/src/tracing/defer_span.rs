use std::fmt;

use eyre::{eyre, OptionExt};
use opentelemetry_sdk::trace::{SpanData, SpanExporter, SpanProcessor};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::core::async_loop::AsyncLoop;

pub struct DeferSpanProcessor {
    otlp_loop: AsyncLoop<()>,
    record_tx: UnboundedSender<SpanData>,
    record_rx: Option<UnboundedReceiver<SpanData>>,
}

impl DeferSpanProcessor {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            otlp_loop: AsyncLoop::new(),
            record_tx: tx,
            record_rx: Some(rx),
        }
    }

    pub fn start(&mut self, exporter: impl SpanExporter + 'static) -> eyre::Result<()> {
        let mut record_rx = self
            .record_rx
            .take()
            .ok_or_eyre("Cannot start defer log processor")?;

        self.otlp_loop.start(async move |cancel_token| {
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(span) = record_rx.recv() => {
                        let result = exporter.export(vec![span]).await;
                        result.expect("Failed to export log message");
                    }
                }
            }
            ()
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> eyre::Result<()> {
        self.otlp_loop.stop().await.map_err(|e| eyre!("{:?}", e))
    }
}

impl SpanProcessor for DeferSpanProcessor {
    fn on_end(&self, span: SpanData) {
        self.record_tx
            .send(span)
            .expect("Failed to send oltp record");
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }

    fn on_start(&self, span: &mut opentelemetry_sdk::trace::Span, cx: &opentelemetry::Context) {
        // Ignored
    }

    fn shutdown(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.otlp_loop.signal_stop();
        Ok(())
    }

    fn shutdown_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> opentelemetry_sdk::error::OTelSdkResult {
        self.otlp_loop.signal_stop();
        Ok(())
    }
}

impl fmt::Debug for DeferSpanProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferLogProcessor").finish()
    }
}
