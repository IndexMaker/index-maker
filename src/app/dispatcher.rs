use eyre::Result;
use std::collections::HashMap;
use std::sync::Arc;
use symm_core::core::bits::Symbol;
use symm_core::core::functional::{PublishSingle, SingleObserver};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

/// Events emitted by dispatcher threads during their lifecycle
#[derive(Debug, Clone)]
pub enum DispatcherEvent {
    ThreadStarting { name: Symbol },
    ThreadStarted { name: Symbol },
    ThreadStopping { name: Symbol },
    ThreadStopped { name: Symbol },
    ThreadError { name: Symbol, error: String },
    MarketDataReceived { symbol: Symbol, price: String },
    OrderProcessed { order_id: String, status: String },
    ShutdownRequested,
    ShutdownComplete,
}

/// Trait for dispatcher implementations
pub trait Dispatcher: Send + Sync {
    /// Get the dispatcher name
    fn name(&self) -> Symbol;
    
    /// Start the dispatcher
    async fn start(&mut self) -> Result<()>;
    
    /// Stop the dispatcher gracefully
    async fn stop(&mut self) -> Result<()>;
    
    /// Check if the dispatcher is running
    fn is_running(&self) -> bool;
    
    /// Set event observer for lifecycle notifications
    fn set_event_observer(&mut self, observer: SingleObserver<DispatcherEvent>);
}

/// Market data dispatcher for handling market data events
pub struct MarketDataDispatcher {
    name: Symbol,
    is_running: bool,
    event_observer: Option<SingleObserver<DispatcherEvent>>,
    task_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
    symbols: Vec<Symbol>,
    update_interval: Duration,
}

impl MarketDataDispatcher {
    pub fn new() -> Self {
        Self {
            name: Symbol::from("MarketData"),
            is_running: false,
            event_observer: None,
            task_handle: None,
            shutdown_tx: None,
            symbols: vec![
                Symbol::from("BTCUSDC"),
                Symbol::from("ETHUSDC"),
                Symbol::from("SOLUSDC"),
            ],
            update_interval: Duration::from_millis(100),
        }
    }

    pub fn with_symbols(mut self, symbols: Vec<Symbol>) -> Self {
        self.symbols = symbols;
        self
    }

    pub fn with_update_interval(mut self, interval: Duration) -> Self {
        self.update_interval = interval;
        self
    }
    
    fn emit_event(&self, event: DispatcherEvent) {
        if let Some(ref observer) = self.event_observer {
            observer.publish_single(event);
        }
    }
}

impl Dispatcher for MarketDataDispatcher {
    fn name(&self) -> Symbol {
        self.name.clone()
    }
    
    async fn start(&mut self) -> Result<()> {
        if self.is_running {
            return Ok(());
        }

        let name = self.name();
        self.emit_event(DispatcherEvent::ThreadStarting { name: name.clone() });

        tracing::info!(dispatcher = %name, symbols = ?self.symbols, "Starting market data dispatcher");

        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
        self.shutdown_tx = Some(shutdown_tx);

        let symbols = self.symbols.clone();
        let update_interval = self.update_interval;
        let event_observer = self.event_observer.clone();
        let dispatcher_name = name.clone();

        let handle = tokio::spawn(async move {
            let mut last_update = Instant::now();
            let mut price_counter = 100.0;

            tracing::info!(dispatcher = %dispatcher_name, "Market data dispatcher thread started");

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::info!(dispatcher = %dispatcher_name, "Market data dispatcher received shutdown signal");
                        break;
                    }
                    _ = tokio::time::sleep_until(last_update + update_interval) => {
                        last_update = Instant::now();

                        // Simulate market data updates
                        symbols
                            .iter()
                            .enumerate()
                            .for_each(|(i, symbol)| {
                                let price = format!("{:.2}", price_counter + (i as f64 * 10.0));

                                if let Some(ref observer) = event_observer {
                                    observer.publish_single(DispatcherEvent::MarketDataReceived {
                                        symbol: symbol.clone(),
                                        price: price.clone(),
                                    });
                                }

                                tracing::trace!(
                                    dispatcher = %dispatcher_name,
                                    symbol = %symbol,
                                    price = %price,
                                    "Market data update"
                                );
                            });

                        price_counter += 0.1;
                    }
                }
            }

            tracing::info!(dispatcher = %dispatcher_name, "Market data dispatcher thread stopped");
        });

        self.task_handle = Some(handle);
        self.is_running = true;

        self.emit_event(DispatcherEvent::ThreadStarted { name: name.clone() });
        tracing::info!(dispatcher = %name, "Market data dispatcher started successfully");

        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        if !self.is_running {
            return Ok(());
        }
        
        let name = self.name();
        self.emit_event(DispatcherEvent::ThreadStopping { name: name.clone() });
        
        tracing::info!(dispatcher = %name, "Stopping market data dispatcher");
        
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        
        self.is_running = false;
        self.emit_event(DispatcherEvent::ThreadStopped { name: name.clone() });

        tracing::info!(dispatcher = %name, "Market data dispatcher stopped successfully");
        
        Ok(())
    }
    
    fn is_running(&self) -> bool {
        self.is_running
    }
    
    fn set_event_observer(&mut self, observer: SingleObserver<DispatcherEvent>) {
        self.event_observer = Some(observer);
    }
}

/// Order processing dispatcher for handling order events
pub struct OrderDispatcher {
    name: Symbol,
    is_running: bool,
    event_observer: Option<SingleObserver<DispatcherEvent>>,
    task_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
    processing_interval: Duration,
    order_queue: Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl OrderDispatcher {
    pub fn new() -> Self {
        Self {
            name: Symbol::from("OrderProcessor"),
            is_running: false,
            event_observer: None,
            task_handle: None,
            shutdown_tx: None,
            processing_interval: Duration::from_millis(50),
            order_queue: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn with_processing_interval(mut self, interval: Duration) -> Self {
        self.processing_interval = interval;
        self
    }

    pub async fn add_order(&self, order_id: String) -> Result<()> {
        let mut queue = self.order_queue.lock().await;
        queue.push(order_id);
        Ok(())
    }
    
    fn emit_event(&self, event: DispatcherEvent) {
        if let Some(ref observer) = self.event_observer {
            observer.publish_single(event);
        }
    }
}

impl Dispatcher for OrderDispatcher {
    fn name(&self) -> Symbol {
        self.name.clone()
    }
    
    async fn start(&mut self) -> Result<()> {
        if self.is_running {
            return Ok(());
        }

        let name = self.name();
        self.emit_event(DispatcherEvent::ThreadStarting { name: name.clone() });

        tracing::info!(dispatcher = %name, "Starting order dispatcher");

        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
        self.shutdown_tx = Some(shutdown_tx);

        let processing_interval = self.processing_interval;
        let order_queue = self.order_queue.clone();
        let event_observer = self.event_observer.clone();
        let dispatcher_name = name.clone();

        let handle = tokio::spawn(async move {
            let mut last_process = Instant::now();

            tracing::info!(dispatcher = %dispatcher_name, "Order dispatcher thread started");

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::info!(dispatcher = %dispatcher_name, "Order dispatcher received shutdown signal");
                        break;
                    }
                    _ = tokio::time::sleep_until(last_process + processing_interval) => {
                        last_process = Instant::now();

                        // Process orders from queue
                        let mut queue = order_queue.lock().await;
                        let orders_to_process = queue.drain(..).collect::<Vec<_>>();
                        drop(queue);

                        orders_to_process
                            .iter()
                            .for_each(|order_id| {
                                let status = String::from("PROCESSED");

                                if let Some(ref observer) = event_observer {
                                    observer.publish_single(DispatcherEvent::OrderProcessed {
                                        order_id: order_id.clone(),
                                        status: status.clone(),
                                    });
                                }

                                tracing::debug!(
                                    dispatcher = %dispatcher_name,
                                    order_id = %order_id,
                                    status = %status,
                                    "Order processed"
                                );
                            });

                        if !orders_to_process.is_empty() {
                            tracing::info!(
                                dispatcher = %dispatcher_name,
                                count = orders_to_process.len(),
                                "Processed orders batch"
                            );
                        }
                    }
                }
            }

            tracing::info!(dispatcher = %dispatcher_name, "Order dispatcher thread stopped");
        });

        self.task_handle = Some(handle);
        self.is_running = true;

        self.emit_event(DispatcherEvent::ThreadStarted { name: name.clone() });
        tracing::info!(dispatcher = %name, "Order dispatcher started successfully");

        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        if !self.is_running {
            return Ok(());
        }
        
        let name = self.name();
        self.emit_event(DispatcherEvent::ThreadStopping { name: name.clone() });
        
        tracing::info!(dispatcher = %name, "Stopping order dispatcher");
        
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        
        self.is_running = false;
        self.emit_event(DispatcherEvent::ThreadStopped { name: name.clone() });

        tracing::info!(dispatcher = %name, "Order dispatcher stopped successfully");
        
        Ok(())
    }
    
    fn is_running(&self) -> bool {
        self.is_running
    }
    
    fn set_event_observer(&mut self, observer: SingleObserver<DispatcherEvent>) {
        self.event_observer = Some(observer);
    }
}

/// Concrete dispatcher types to avoid trait object issues
pub enum ConcreteDispatcher {
    MarketData(MarketDataDispatcher),
    Order(OrderDispatcher),
}

impl ConcreteDispatcher {
    pub fn name(&self) -> Symbol {
        match self {
            ConcreteDispatcher::MarketData(d) => d.name(),
            ConcreteDispatcher::Order(d) => d.name(),
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        match self {
            ConcreteDispatcher::MarketData(d) => d.start().await,
            ConcreteDispatcher::Order(d) => d.start().await,
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self {
            ConcreteDispatcher::MarketData(d) => d.stop().await,
            ConcreteDispatcher::Order(d) => d.stop().await,
        }
    }

    pub fn is_running(&self) -> bool {
        match self {
            ConcreteDispatcher::MarketData(d) => d.is_running(),
            ConcreteDispatcher::Order(d) => d.is_running(),
        }
    }

    pub fn set_event_observer(&mut self, observer: SingleObserver<DispatcherEvent>) {
        match self {
            ConcreteDispatcher::MarketData(d) => d.set_event_observer(observer),
            ConcreteDispatcher::Order(d) => d.set_event_observer(observer),
        }
    }
}

/// Dispatcher manager that coordinates all background dispatchers
pub struct DispatcherManager {
    dispatchers: HashMap<Symbol, ConcreteDispatcher>,
    event_observer: Option<SingleObserver<DispatcherEvent>>,
    is_running: bool,
}

impl DispatcherManager {
    /// Create a new dispatcher manager
    pub fn new() -> Self {
        Self {
            dispatchers: HashMap::new(),
            event_observer: None,
            is_running: false,
        }
    }

    /// Add a dispatcher to the manager
    pub fn add_dispatcher(&mut self, dispatcher: ConcreteDispatcher) {
        let name = dispatcher.name();

        // Set event observer if we have one
        if let Some(ref _observer) = self.event_observer {
            // Note: Event observer setup is handled when set_event_observer is called
            // on the manager, which will propagate to all dispatchers
        }

        self.dispatchers.insert(name.clone(), dispatcher);

        tracing::info!(dispatcher = %name, "Added dispatcher to manager");
    }

    /// Set event observer for all dispatchers
    pub fn set_event_observer(&mut self, observer: SingleObserver<DispatcherEvent>) {
        self.event_observer = Some(observer);

        // Note: Individual dispatcher observers are managed separately
        // Each dispatcher will emit events that can be captured by the manager's observer
        tracing::debug!("Event observer set for dispatcher manager");
    }

    /// Start all dispatchers
    pub async fn start_all(&mut self) -> Result<()> {
        if self.is_running {
            return Ok(());
        }

        tracing::info!(count = self.dispatchers.len(), "Starting all dispatchers");

        self.dispatchers
            .iter_mut()
            .map(|(name, dispatcher)| {
                tracing::info!(dispatcher = %name, "Starting dispatcher");
                dispatcher.start().map_err(|e| {
                    let error_msg = format!("Failed to start dispatcher {}: {:?}", name, e);
                    if let Some(ref observer) = self.event_observer {
                        observer.publish_single(DispatcherEvent::ThreadError {
                            name: name.clone(),
                            error: error_msg.clone()
                        });
                    }
                    eyre::eyre!(error_msg)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.is_running = true;
        tracing::info!("All dispatchers started successfully");

        Ok(())
    }

    /// Stop all dispatchers gracefully
    pub async fn stop_all(&mut self) -> Result<()> {
        if !self.is_running {
            return Ok(());
        }

        tracing::info!(count = self.dispatchers.len(), "Stopping all dispatchers");

        if let Some(ref observer) = self.event_observer {
            observer.publish_single(DispatcherEvent::ShutdownRequested);
        }

        self.dispatchers
            .iter_mut()
            .map(|(name, dispatcher)| {
                tracing::info!(dispatcher = %name, "Stopping dispatcher");
                dispatcher.stop().map_err(|e| {
                    let error_msg = format!("Failed to stop dispatcher {}: {:?}", name, e);
                    if let Some(ref observer) = self.event_observer {
                        observer.publish_single(DispatcherEvent::ThreadError {
                            name: name.clone(),
                            error: error_msg.clone()
                        });
                    }
                    eyre::eyre!(error_msg)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.is_running = false;

        if let Some(ref observer) = self.event_observer {
            observer.publish_single(DispatcherEvent::ShutdownComplete);
        }

        tracing::info!("All dispatchers stopped successfully");

        Ok(())
    }

    /// Check if all dispatchers are running
    pub fn is_running(&self) -> bool {
        self.is_running && self.dispatchers.values().all(|d| d.is_running())
    }

    /// Get dispatcher count
    pub fn dispatcher_count(&self) -> usize {
        self.dispatchers.len()
    }

    /// Create a default dispatcher manager with standard dispatchers
    pub fn with_default_dispatchers() -> Self {
        let mut manager = Self::new();

        // Add standard dispatchers
        manager.add_dispatcher(ConcreteDispatcher::MarketData(MarketDataDispatcher::new()));
        manager.add_dispatcher(ConcreteDispatcher::Order(OrderDispatcher::new()));

        tracing::info!(count = manager.dispatcher_count(), "Created dispatcher manager with default dispatchers");

        manager
    }
}
