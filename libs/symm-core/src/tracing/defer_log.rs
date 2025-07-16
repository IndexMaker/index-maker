use std::fmt;

use eyre::{eyre, OptionExt};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::logs::{LogBatch, LogExporter, LogProcessor, SdkLogRecord};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::core::async_loop::AsyncLoop;

pub struct DeferLogProcessor {
    otlp_loop: AsyncLoop<()>,
    record_tx: UnboundedSender<(SdkLogRecord, InstrumentationScope)>,
    record_rx: Option<UnboundedReceiver<(SdkLogRecord, InstrumentationScope)>>,
}

impl DeferLogProcessor {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            otlp_loop: AsyncLoop::new(),
            record_tx: tx,
            record_rx: Some(rx),
        }
    }

    pub fn start(&mut self, exporter: impl LogExporter + 'static) -> eyre::Result<()> {
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
                    Some((ref record, ref instrumentation)) = record_rx.recv() => {
                        let log_tuple = &[(record, instrumentation)];
                        let result = exporter.export(LogBatch::new(log_tuple)).await;
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

impl LogProcessor for DeferLogProcessor {
    fn emit(
        &self,
        record: &mut opentelemetry_sdk::logs::SdkLogRecord,
        instrumentation: &opentelemetry::InstrumentationScope,
    ) {
        let val = (record.clone(), instrumentation.clone());
        self.record_tx
            .send(val)
            .expect("Failed to send oltp record");
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
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
