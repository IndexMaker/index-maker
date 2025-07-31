use std::fmt;

use eyre::{eyre, OptionExt};
use itertools::Itertools;
use opentelemetry_sdk::trace::{SpanData, SpanExporter, SpanProcessor};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::core::async_loop::AsyncLoop;

enum DeferSpanEvent {
    SpanEnded(SpanData),
    ForceFlush,
}

pub struct DeferSpanProcessor {
    otlp_loop: AsyncLoop<()>,
    record_tx: UnboundedSender<DeferSpanEvent>,
    record_rx: Option<UnboundedReceiver<DeferSpanEvent>>,
    batch_size: usize,
}

impl DeferSpanProcessor {
    pub fn new(batch_size: usize) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            otlp_loop: AsyncLoop::new(),
            record_tx: tx,
            record_rx: Some(rx),
            batch_size,
        }
    }

    pub fn start(&mut self, exporter: impl SpanExporter + 'static) -> eyre::Result<()> {
        let mut record_rx = self
            .record_rx
            .take()
            .ok_or_eyre("Cannot start defer log processor")?;

        let batch_size = self.batch_size;

        self.otlp_loop.start(async move |cancel_token| {
            let mut cache = Vec::new();
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(event) = record_rx.recv() => {
                        match event {
                            DeferSpanEvent::SpanEnded(span) => {
                                cache.push(span);
                                if cache.len() == batch_size {
                                    let _ = exporter.export(cache.drain(..).collect_vec()).await;
                                }
                            },
                            DeferSpanEvent::ForceFlush => {
                                let _ = exporter.export(cache.drain(..).collect_vec()).await;
                            }
                        }
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
        let _ = self.record_tx.send(DeferSpanEvent::SpanEnded(span));
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        let _ = self.record_tx.send(DeferSpanEvent::ForceFlush);
        Ok(())
    }

    fn on_start(&self, _span: &mut opentelemetry_sdk::trace::Span, _cx: &opentelemetry::Context) {
        // Ignored
    }

    fn shutdown(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.otlp_loop.signal_stop();
        Ok(())
    }

    fn shutdown_with_timeout(
        &self,
        _timeout: std::time::Duration,
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
