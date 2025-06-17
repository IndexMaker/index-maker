use std::sync::Arc;

use parking_lot::RwLock;

/// Every notification is handled only once, and so then can be moved!
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
    fn get_multi_observer_arc(&mut self) -> &Arc<RwLock<MultiObserver<T>>>;
}

pub mod crossbeam {
    use std::any::type_name;

    use crossbeam::channel::Sender;

    use crate::core::functional::{IntoNotificationHandlerOnceBox, NotificationHandlerOnce};

    impl<T> NotificationHandlerOnce<T> for Sender<T>
    where
        T: Send + Sync,
    {
        fn handle_notification(&self, notification: T) {
            self.send(notification)
                .expect(format!("Failed to handle {}", type_name::<T>()).as_str());
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
