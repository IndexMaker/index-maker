use core::fmt;
use std::sync::Arc;

use parking_lot::RwLock;

// This is a case when both the observer and the event are consumed
pub trait OneShotNotificationHandlerOnce<T>: Send + Sync {
    fn handle_notification(self: Box<Self>, notification: T);
}

impl<F, T> OneShotNotificationHandlerOnce<T> for F
where
    F: FnOnce(T) + Send + Sync,
{
    fn handle_notification(self: Box<Self>, notification: T) {
        (self)(notification)
    }
}

pub trait IntoOneShotNotificationHandlerOnceBox<T> {
    fn into_one_shot_notification_handler_once_box(
        self,
    ) -> Box<dyn OneShotNotificationHandlerOnce<T>>;
}

pub trait OneShotPublishSingle<T> {
    fn one_shot_publish_single(self, notification: T);
}

pub struct OneShotSingleObserver<T> {
    observer: Option<Box<dyn OneShotNotificationHandlerOnce<T>>>,
}

// We use one shot for asynchronous replies to commands, and
// commands usually implement Debug.
impl<T> fmt::Debug for OneShotSingleObserver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OneShotSingleObserver(<user function>)")
    }
}

impl<T> OneShotSingleObserver<T> {
    pub fn new() -> Self {
        Self { observer: None }
    }

    pub fn new_with_observer(observer: Box<dyn OneShotNotificationHandlerOnce<T>>) -> Self {
        Self {
            observer: Some(observer),
        }
    }

    pub fn new_with_fn(observer: impl OneShotNotificationHandlerOnce<T> + 'static) -> Self {
        Self::new_with_observer(Box::new(observer))
    }

    pub fn new_from(value: impl IntoOneShotNotificationHandlerOnceBox<T>) -> Self {
        Self::new_with_observer(value.into_one_shot_notification_handler_once_box())
    }

    pub fn has_observer(&self) -> bool {
        self.observer.is_some()
    }
}

impl<T> OneShotPublishSingle<T> for OneShotSingleObserver<T> {
    fn one_shot_publish_single(self, notification: T) {
        if let Some(observer) = self.observer {
            observer.handle_notification(notification);
        }
    }
}

/// Every notification is handled only once, and so then can be moved!
/// This is the case when observer is reused for next events, and
/// events are consumed (they live only once)
pub trait NotificationHandlerOnce<T>: Send + Sync {
    fn handle_notification(&self, notification: T);
}

impl<F, T> NotificationHandlerOnce<T> for F
where
    F: Fn(T) + Send + Sync,
{
    fn handle_notification(&self, notification: T) {
        (self)(notification)
    }
}

pub trait IntoNotificationHandlerOnceBox<T> {
    fn into_notification_handler_once_box(self) -> Box<dyn NotificationHandlerOnce<T>>;
}

pub trait PublishSingle<T> {
    fn publish_single(&self, notification: T);
}

pub struct SingleObserver<T> {
    observer: Option<Box<dyn NotificationHandlerOnce<T>>>,
}

impl<T> SingleObserver<T> {
    pub fn new() -> Self {
        Self { observer: None }
    }

    pub fn new_with_observer(observer: Box<dyn NotificationHandlerOnce<T>>) -> Self {
        Self {
            observer: Some(observer),
        }
    }

    pub fn new_with_fn(observer: impl NotificationHandlerOnce<T> + 'static) -> Self {
        Self::new_with_observer(Box::new(observer))
    }

    pub fn new_from(value: impl IntoNotificationHandlerOnceBox<T>) -> Self {
        Self::new_with_observer(value.into_notification_handler_once_box())
    }

    pub fn has_observer(&self) -> bool {
        self.observer.is_some()
    }

    /// There is only single observer that can be set, and so we call it 'set'
    pub fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<T>>) {
        self.observer = Some(observer);
    }

    pub fn set_observer_fn(&mut self, observer: impl NotificationHandlerOnce<T> + 'static) {
        self.set_observer(Box::new(observer));
    }

    pub fn set_observer_from(&mut self, observer: impl IntoNotificationHandlerOnceBox<T>) {
        self.observer = Some(observer.into_notification_handler_once_box());
    }
}

impl<T> PublishSingle<T> for SingleObserver<T> {
    /// There will be only one handler, and so we call it 'single'
    fn publish_single(&self, notification: T) {
        if let Some(observer) = &self.observer {
            observer.handle_notification(notification);
        }
    }
}

pub trait IntoObservableSingle<T>: Send + Sync {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<T>;
}

pub trait IntoObservableSingleArc<T>: Send + Sync {
    fn get_single_observer_arc(&mut self) -> &Arc<RwLock<SingleObserver<T>>>;
}

pub trait IntoObservableSingleVTable<T>: Send + Sync {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<T>>);
}

pub trait IntoObservableSingleFun<T>: Send + Sync {
    fn set_observer_fn(&mut self, observer: impl NotificationHandlerOnce<T> + 'static);
    fn set_observer_from(&mut self, observer: impl IntoNotificationHandlerOnceBox<T>);
}

impl<A, T> IntoObservableSingleFun<T> for A
where
    A: IntoObservableSingleVTable<T> + ?Sized,
{
    fn set_observer_fn(&mut self, observer: impl NotificationHandlerOnce<T> + 'static) {
        self.set_observer(Box::new(observer));
    }

    fn set_observer_from(&mut self, observer: impl IntoNotificationHandlerOnceBox<T>) {
        self.set_observer(observer.into_notification_handler_once_box());
    }
}

/// Notifications can be handled by multiple handler, and so they must be passed
/// by reference
pub trait NotificationHandler<T>: Send + Sync {
    fn handle_notification(&self, notification: &T);
}

impl<F, T> NotificationHandler<T> for F
where
    F: Fn(&T) + Send + Sync,
{
    fn handle_notification(&self, notification: &T) {
        (self)(notification)
    }
}

pub trait IntoNotificationHandlerBox<T> {
    fn into_notification_handler_box(self) -> Box<dyn NotificationHandler<T>>;
}

pub trait PublishMany<T> {
    fn publish_many(&self, notification: &T);
}

pub struct MultiObserver<T> {
    observers: Vec<Box<dyn NotificationHandler<T>>>,
}

impl<T> MultiObserver<T> {
    pub fn new() -> Self {
        Self { observers: vec![] }
    }

    pub fn has_observers(&self) -> bool {
        !self.observers.is_empty()
    }

    pub fn new_with_observers(observers: Vec<Box<dyn NotificationHandler<T>>>) -> Self {
        Self { observers }
    }

    pub fn new_from(value: impl IntoNotificationHandlerBox<T>) -> Self {
        Self::new_with_observers(vec![value.into_notification_handler_box()])
    }

    /// There can be multiple observers, and so we call it 'add'
    pub fn add_observer(&mut self, observer: Box<dyn NotificationHandler<T>>) {
        self.observers.push(observer);
    }

    pub fn add_observer_fn(&mut self, observer: impl NotificationHandler<T> + 'static) {
        self.observers.push(Box::new(observer));
    }

    pub fn add_observer_from(&mut self, observer: impl IntoNotificationHandlerBox<T>) {
        self.observers
            .push(observer.into_notification_handler_box());
    }
}

impl<T> PublishMany<T> for MultiObserver<T> {
    /// There can be multiple observers, and so we call it 'many'
    fn publish_many(&self, notification: &T) {
        for observer in &self.observers {
            observer.handle_notification(notification);
        }
    }
}

pub trait IntoObservableMany<T>: Send + Sync {
    fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<T>;
}

pub trait IntoObservableManyArc<T>: Send + Sync {
    fn get_multi_observer_arc(&self) -> &Arc<RwLock<MultiObserver<T>>>;
}

pub trait IntoObservableManyVTable<T>: Send + Sync {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<T>>);
}

pub trait IntoObservableManyFun<T>: Send + Sync {
    fn add_observer_fn(&mut self, observer: impl NotificationHandler<T> + 'static);
    fn add_observer_from(&mut self, observer: impl IntoNotificationHandlerBox<T>);
}

impl<A, T> IntoObservableManyFun<T> for A
where
    A: IntoObservableManyVTable<T> + ?Sized,
{
    fn add_observer_fn(&mut self, observer: impl NotificationHandler<T> + 'static) {
        self.add_observer(Box::new(observer));
    }

    fn add_observer_from(&mut self, observer: impl IntoNotificationHandlerBox<T>) {
        self.add_observer(observer.into_notification_handler_box());
    }
}

pub mod crossbeam {
    use std::any::type_name;

    use crossbeam::channel::Sender;

    use crate::core::functional::{
        IntoNotificationHandlerBox, IntoNotificationHandlerOnceBox,
        IntoOneShotNotificationHandlerOnceBox, NotificationHandler, NotificationHandlerOnce,
        OneShotNotificationHandlerOnce,
    };

    impl<T> OneShotNotificationHandlerOnce<T> for Sender<T>
    where
        T: Send + Sync,
    {
        fn handle_notification(self: Box<Self>, notification: T) {
            if let Err(err) = self.send(notification) {
                tracing::warn!("Failed to send {}: {:?}", type_name::<T>(), err);
            }
        }
    }

    impl<T> IntoOneShotNotificationHandlerOnceBox<T> for Sender<T>
    where
        T: Send + Sync + 'static,
    {
        fn into_one_shot_notification_handler_once_box(
            self,
        ) -> Box<dyn OneShotNotificationHandlerOnce<T>> {
            Box::new(self)
        }
    }

    impl<T> NotificationHandlerOnce<T> for Sender<T>
    where
        T: Send + Sync,
    {
        fn handle_notification(&self, notification: T) {
            if let Err(err) = self.send(notification) {
                tracing::warn!("Failed to send {}: {:?}", type_name::<T>(), err);
            }
        }
    }

    impl<T> IntoNotificationHandlerOnceBox<T> for Sender<T>
    where
        T: Send + Sync + 'static,
    {
        fn into_notification_handler_once_box(self) -> Box<dyn NotificationHandlerOnce<T>> {
            Box::new(self)
        }
    }

    impl<T> NotificationHandler<T> for Sender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        fn handle_notification(&self, notification: &T) {
            if let Err(err) = self.send(notification.clone()) {
                tracing::warn!("Failed to send {}: {:?}", type_name::<T>(), err);
            }
        }
    }
    impl<T> IntoNotificationHandlerBox<T> for Sender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        fn into_notification_handler_box(self) -> Box<dyn NotificationHandler<T>> {
            Box::new(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use crate::core::functional::{MultiObserver, PublishMany, PublishSingle};

    use super::{NotificationHandlerOnce, SingleObserver};

    #[test]
    fn test_notification_handler_fn() {
        let _: Box<dyn NotificationHandlerOnce<i32>> = Box::new(|_: i32| {});
    }

    #[test]
    fn test_single_observer_1() {
        let (tx, rx) = channel::<i32>();
        let mut observer = SingleObserver::<i32>::new();
        observer.set_observer(Box::new(move |x: i32| {
            assert_eq!(tx.send(x), Ok(()));
        }));
        observer.publish_single(10);
        assert_eq!(10, rx.recv().unwrap());
    }

    #[test]
    fn test_single_observer_2() {
        let (tx, rx) = channel::<i32>();
        let observer = SingleObserver::<i32>::new_with_observer(Box::new(move |x: i32| {
            assert_eq!(tx.send(x), Ok(()));
        }));
        observer.publish_single(10);
        assert_eq!(10, rx.recv().unwrap());
    }

    #[test]
    fn test_multi_observer_1() {
        let (tx, rx) = channel::<i32>();
        let mut observer = MultiObserver::<i32>::new();
        observer.add_observer(Box::new(move |x: &i32| {
            assert_eq!(tx.send(*x), Ok(()));
        }));
        observer.publish_many(&10);
        assert_eq!(10, rx.recv().unwrap());
    }

    #[test]
    fn test_multi_observer_2() {
        let (tx, rx) = channel::<i32>();
        let observer = MultiObserver::<i32>::new_with_observers(vec![Box::new(move |x: &i32| {
            assert_eq!(tx.send(*x), Ok(()));
        })]);
        observer.publish_many(&10);
        assert_eq!(10, rx.recv().unwrap());
    }
}
