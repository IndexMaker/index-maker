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
* [x] Index Order should use Collateral Amount instead of Price & Quantity
    - Change New Order data structures
    - Change liquidity fitting to fit collateral
* [x] Collateral Management (Buy side)
    - Obtain collateral management details from Solver
    - Route collateral from different chains to single destination
    - Track collateral journey using lots accounting
    - Reserve confirmed collateral for Solver to start creating order batches
* [x] Manage Order Batches correctly
    - [x] Collapse multiple orders for same assets in batches
    - [x] Carry over remaining collateral between batches
    - [x] Carry over unused batch order lots between batches
    - [x] Report completed batches and mintable orders using events
    - [x] Remove fully-filled batches
* [x] Manage Index Orders correctly
    - [x] After minting remove Index Order
    - [x] Support Index Order updates
    - [x] Fill correct Index Order update based on client order ID
    - [x] Notify index order manager of lots assigned to index orders
* [ ] Add Unit Tests
    - [ ] Write unit tests for Simple Solver
    - [ ] Write unit tests for Batch Manager
    - [ ] Write unit tests for Collateral Manager
* [ ] Quote Requests
    - [ ] Manage Quote Requests
    - [ ] Compute Quotes
* [ ] Sell side implementation
    - [ ] Wait for index token collateral before selling assets
    - [ ] Obtain USD cash management details after filling index order
    - [ ] Confirm USD cash before withdrawal
    - [ ] Burn index token collateral
* [ ] Implement additional limits
    - [ ] Ensure individual orders in a batch have size at least minimum
    - [ ] Ensure order rate is within limits
    - [ ] Ensure total volley size across batches is not exceeded
* [ ] Logging & Error handling
    - [ ] Find out best way to handle runtime errors
    - [ ] Explore logging mechanism, and craft log messages
* [ ] Improve SBE Test Scenario
    - [ ] Convert SBE test scenario into a framework for scenario testing
    - [ ] Provide multiple test scenarios
    - [ ] Hone main scenario to produce human friendly numbers
* [ ] Improve Solver Strategy
    - [ ] Investigate behaviour of SimpleSolver in various scenarios
    - [ ] Adjust SimpleSolver liquiditiy fitting algoithm
    - [ ] See if MatrixSolver would be beneficial
* [ ] Improve Collateral Routing
    - [ ] Remove closed lots
    - [ ] Investigate what happens when there is both a deposit for Buy trade, and a withdrawal for Sell
    - [ ] Multiple destinations
    - [ ] Create a relationship between destinations and Order Connector

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

## Phase 2. FIX integration

### Goal

The goal is to have fully functional FIX server capable of receiving NewOrder, CancelOrder etc messages

## Phase 3. Chain integration

### Goal

The goal is to have fully functiona chain integration so that we can receive on-chain events and invoke on-chain smart-contract methods.

## Phase 4. Binance Market Data integration

### Goal

The goal is to integrate market data from Binance, and observe system behavior and accuracy dependant on changing market conditions.
We would be sending orders into mock connector, yet using real market data. This is to confirm that orders look as expected and system
works correctly.

## Phase 5. Binance Orders integration

### Goal

The goal is to integrate order sending with Binance, and to conrifm that orders are sent, they don't exceed limits, they get
executed, we track them correctly all the way up until minting.

## Phase X. Future Development

After delivering working system, which is capable of accepting Index Orders, and sending order batches to Binance, we will
look into robustness of the solution, performance, scalling, fixing bugs and adding more features.





