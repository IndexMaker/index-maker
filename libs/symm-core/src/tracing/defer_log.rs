use std::fmt;

use eyre::{eyre, OptionExt};
use itertools::Itertools;
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::logs::{LogBatch, LogExporter, LogProcessor, SdkLogRecord};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::core::async_loop::AsyncLoop;

enum DeferLogEvent {
    Emit(Box<(SdkLogRecord, InstrumentationScope)>),
    ForceFlush,
}

pub struct DeferLogProcessor {
    otlp_loop: AsyncLoop<()>,
    record_tx: UnboundedSender<DeferLogEvent>,
    record_rx: Option<UnboundedReceiver<DeferLogEvent>>,
    batch_size: usize,
}

impl DeferLogProcessor {
    pub fn new(batch_size: usize) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            otlp_loop: AsyncLoop::new(),
            record_tx: tx,
            record_rx: Some(rx),
            batch_size,
        }
    }

    pub fn start(&mut self, exporter: impl LogExporter + 'static) -> eyre::Result<()> {
        let mut record_rx = self
            .record_rx
            .take()
            .ok_or_eyre("Cannot start defer log processor")?;

        let batch_size = self.batch_size;

        self.otlp_loop.start(async move |cancel_token| {
            let mut cache = Vec::new();

            let flush = async |cache: &mut Vec<_>| {
                let buf = cache
                    .iter()
                    .map(|x: &Box<(SdkLogRecord, InstrumentationScope)>| (&x.0, &x.1))
                    .collect_vec();
                let _ = exporter.export(LogBatch::new(&buf)).await;
                cache.clear();
            };

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(event) = record_rx.recv() => {
                        match event {
                            DeferLogEvent::Emit(boxed) => {
                                cache.push(boxed);
                                if cache.len() == batch_size {
                                    flush(&mut cache).await;
                                }
                            },
                            DeferLogEvent::ForceFlush => {
                                flush(&mut cache).await;
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

impl LogProcessor for DeferLogProcessor {
    fn emit(
        &self,
        record: &mut opentelemetry_sdk::logs::SdkLogRecord,
        instrumentation: &opentelemetry::InstrumentationScope,
    ) {
        let _ = self.record_tx.send(DeferLogEvent::Emit(Box::new((
            record.clone(),
            instrumentation.clone(),
        ))));
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        let _ = self.record_tx.send(DeferLogEvent::ForceFlush);
        Ok(())
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

impl fmt::Debug for DeferLogProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferLogProcessor").finish()
    }
}
