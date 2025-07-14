use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::Context;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::collections::HashMap;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug, Clone)]
pub struct TracingData {
    properties: HashMap<String, String>,
}

impl TracingData {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
        }
    }
}

pub trait WithTracingData {
    fn get_tracing_data_mut(&mut self) -> &mut TracingData;
    fn get_tracing_data(&self) -> &TracingData;
}

impl Injector for TracingData {
    fn set(&mut self, key: &str, value: String) {
        self.properties.insert(key.to_string(), value);
    }
}

impl Extractor for TracingData {
    fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(|s| s.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.properties.keys().map(|s| s.as_str()).collect()
    }
}

pub trait WithTracingContext {
    fn inject_current_context(&mut self);
    fn extract_context(&self) -> Context;
}

impl<T> WithTracingContext for T
where
    T: WithTracingData,
{
    fn inject_current_context(&mut self) {
        let propagator = TraceContextPropagator::new();
        let parent_context = Span::current().context();
        propagator.inject_context(&parent_context, self.get_tracing_data_mut());
    }

    fn extract_context(&self) -> Context {
        let propagator = TraceContextPropagator::new();
        propagator.extract(self.get_tracing_data())
    }
}

#[derive(Debug, Clone)]
pub struct TraceableEvent<T> {
    notification: T,
    tracing_data: TracingData,
}

impl<T> TraceableEvent<T> {
    pub fn new(notification: T) -> Self {
        Self {
            notification,
            tracing_data: TracingData::new(),
        }
    }

    pub fn take(self) -> (T, Context) {
        let context = self.extract_context();
        (self.notification, context)
    }

    pub fn with_tracing<R>(self, f: impl FnOnce(T) -> R) -> R {
        let (notification, context) = self.take();
        let _guard = context.attach();
        f(notification)
    }
}

impl<T> WithTracingData for TraceableEvent<T> {
    fn get_tracing_data_mut(&mut self) -> &mut TracingData {
        &mut self.tracing_data
    }

    fn get_tracing_data(&self) -> &TracingData {
        &self.tracing_data
    }
}

pub mod crossbeam {
    use std::any::type_name;

    use crossbeam::channel::{unbounded, Receiver, Sender};

    use crate::core::{
        functional::{
            IntoNotificationHandlerBox, IntoNotificationHandlerOnceBox, NotificationHandler,
            NotificationHandlerOnce,
        },
        telemetry::{TraceableEvent, WithTracingContext},
    };

    pub struct TraceableNotificationSender<T> {
        sender: Sender<TraceableEvent<T>>,
    }

    impl<T> TraceableNotificationSender<T> {
        pub fn new(sender: Sender<TraceableEvent<T>>) -> Self {
            Self { sender }
        }
    }

    impl<T> Clone for TraceableNotificationSender<T> {
        fn clone(&self) -> Self {
            Self {
                sender: self.sender.clone(),
            }
        }
    }

    pub trait IntoTraceableNotificationSender<T> {
        fn into_traceable_notification_sender(self) -> TraceableNotificationSender<T>;
    }

    impl<T> IntoTraceableNotificationSender<T> for Sender<TraceableEvent<T>> {
        fn into_traceable_notification_sender(self) -> TraceableNotificationSender<T> {
            TraceableNotificationSender::new(self)
        }
    }

    pub fn unbounded_traceable<T>() -> (TraceableNotificationSender<T>, Receiver<TraceableEvent<T>>)
    {
        let (tx, rx) = unbounded();
        (tx.into_traceable_notification_sender(), rx)
    }

    impl<T> NotificationHandlerOnce<T> for TraceableNotificationSender<T>
    where
        T: Send + Sync + 'static,
    {
        fn handle_notification(&self, notification: T) {
            let mut traced_message = TraceableEvent::new(notification);
            traced_message.inject_current_context();

            if let Err(err) = self.sender.send(traced_message) {
                tracing::warn!("Failed to send {}: {:?}", type_name::<T>(), err);
            }
        }
    }

    impl<T> IntoNotificationHandlerOnceBox<T> for TraceableNotificationSender<T>
    where
        T: Send + Sync + 'static,
    {
        fn into_notification_handler_once_box(self) -> Box<dyn NotificationHandlerOnce<T>> {
            Box::new(self)
        }
    }

    impl<T> NotificationHandler<T> for TraceableNotificationSender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        fn handle_notification(&self, notification: &T) {
            let mut traced_message = TraceableEvent::new(notification.clone());
            traced_message.inject_current_context();

            if let Err(err) = self.sender.send(traced_message) {
                tracing::warn!("Failed to send {}: {:?}", type_name::<T>(), err);
            }
        }
    }

    impl<T> IntoNotificationHandlerBox<T> for TraceableNotificationSender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        fn into_notification_handler_box(self) -> Box<dyn NotificationHandler<T>> {
            Box::new(self)
        }
    }
}
