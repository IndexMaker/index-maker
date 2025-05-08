# Developent Plan

## Phase 1. Mocked I/O (FIX / Chain/ Market Data)

### Goal

The goal of this phase is to:
- Understand the business requirements of the project
- Prepare initial technical design
- Create proof-of-concept (PoC) implementation demonstrating how will the system work

The resulting PoC should be able to demonstrate following activity:
- User sends Collateral
- User sends Index Order
- Solver receives Collateral
- Solver sends asset orders to exchange
- Solver recieves individual Fills
- Solver fills Index Order
- User receives confirmations of each individual Index Order fill
- User receives newly minted Index Token

### Work Items

Work included in Phase 1.:

* [x] Receiving new Index Orders 
    - Mock FIX server publishes New Order message
    - Index Order Manager processes the message compacting orders
    - Solver receives new order message
* [x] Fitting into available Market Liquidity
    - Solver picks new order message, and 
    - Finds available liquidity for assets in the basket of an index in order
    - Calculates quantity that can be executed based on that liquidity
    - Caps order size to available liquidity
* [x] Sending order Batches
    - Solver does all the above in batches of orders as available liquidity as well as order rate to exchange (Binance) needs to be shared across users
    - Batches of orders for individual assets are sent to exchage and tracked
    - We track full lifecycle of each asset order we send to exchage, i.e. when there is an execution or cancel of an individual asset order we update that order status.
* [x] Tracking inventory Positions & Lots
    - We track full lifecycle of each individual lot for each asset that we received from exchange as a result of order execution. This allows us to know what we have in our inventory, when we acquired it, how much has been disposed (sold back on exchange), and where the exact unit prices, quantites, and fees.
* [x] Progressively filling Index Orders from newly Open Lots
    - We fill Index Orders based on individual fills for each individual asset, i.e. whenever new lot is open (we receive an event from Inventory Manager), we will allocate portions of that lot across all Index Orders in the batch proportionally to their contribution.
    - We send back to FIX server fill-report for each individual filled portion of the Index Order.
* [x] Minting Index Tokens
    - Calculate execution price, i.e. We track prices and fees for each individual portion of the Index Order fill, and we sum those up to calculate the execution price of the Index Token.
    - Send request to Mint Token
* [ ] Index Order should use Collateral Amount instead of Price & Quantity
    - Change New Order data structures
    - Change liquidity fitting to fit collateral
    - Collapse multiple orders for same assets in batches

#### Design & Performance

* Logging
    - We need to review our logging, as at present messages and errors are logged to standard output.
    - We need to review log messages, and ensure they contain just right amount of information for us to understand what happened in the system, and not too much to avoid logs bloating and poor overall performance.
    - We need to find logging mechanism that we want to use, and test it so that we know how it performs.
* Error handling
    - We need to review our error handling, as we are currently bailing out in case of an error, and we might be leaving some state inconsistent (especially if arithmetic error happens in the middle of calculations).
* Multi-Threaded Performance
    - We need to review design of queues and events in context of performance and
    maintainability, i.e. we use Mutex and RwLock from parking-lot, and then
    standard VecDeque and HashMap. The VecDeque gives us ability to efficiently
    drain number of elements from the front using some predicate.  The parking-lot
    implementations of Mutex and RwLock use atomics, and acquiring locks is
    relatively cheap. This approach is good, and should give decent performance,
    however under contention those queues will be accessed in serialized fashion,
    i.e. only one thread can read or write from them. We were thinking of using
    channels, or concurrent queue (cross-beam SegQueue). There are also
    implementations of concurrent hash-maps that we could explore.
* Possible ActiX Framework Integration
    - The ActiX framework is renowned for its high performance. Yet, within this
    framework all action happens on a single thread, and communication between
    Actors is done via messaging. It shouldn't be hard to transform current
    event-driven design of the PoC to be integrated with ActiX framework. However
    we're interested in multi-threaded option, for which ActiX also has some
    support.
* Disaster recovery plan
    - We need to understand our system well, make sure logging is 

