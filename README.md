# Developent Plan

## Backlog

### Work Items

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
* [x] Handle Index Order updates correctly
    - [x] Solver must update remaining quantity to reflect Index Order
* [x] Handle Index Order Engagements correctly
    - [x] Index Order Manager should only allow engagements in FIFO
    - [x] Solver must engage Index Order updates in FIFO
* [x] Add Unit Tests
    - [x] Write unit tests for Simple Solver
    - [x] Write unit tests for Batch Manager
    - [x] Write unit tests for Collateral Manager
    - [x] Write unit tests for Collateral Router
    - [x] Write unit tests for Collateral Position
    - [x] Write unit tests for Solver Client Orders
* [x] Quote Requests
    - [x] Manage Quote Requests
    - [x] Compute Quotes
* [ ] FIX Server Investigation
    - [x] Investigate Actix vs Axum web frameworks
    - [x] Investigate FIX server REST
    - [x] Investigate WebSockets
    - [x] Design FIX server architecture
    - [x] Create FIX server implementation
    - [x] Create FIX server integration example
    - [ ] FIX message signed by private wallet key
* [x] Binance Market Data Connector
    - [x] Investigate work required to support Binance Market Data
    - [x] Receive book deltas and snapshots from Binance
    - [x] Design Binance Market Data Connector architecture
    - [x] Create Binance Market Data Connector implementation
    - [x] Create Binance Market Data Connector integration example
    - [x] Build order book from Binance market data events
* [x] Binance Order Sending Connector
    - [x] Investigate work required to support sending orders to Binance
    - [x] Design Binance Order Connector architecture
    - [x] Create Binance Order Connector implementation
    - [x] Create Binance Order Connector integration example
    - [x] Send small order to Binance using limit price X% away from TOB
* [ ] Bitget Market Data Connector
    - [x] Investigate work required to support Bitget Market Data
    - [x] Receive book deltas and snapshots from Bitget
    - [ ] Design Bitget Market Data Connector architecture
    - [ ] Create Bitget Market Data Connector implementation
    - [ ] Create Bitget Market Data Connector integration example
    - [ ] Build order book from Bitget market data events
* [ ] Bitget Order Sending Connector
    - [x] Investigate work required to support sending orders to Bitget
    - [ ] Design Bitget Order Connector architecture
    - [ ] Create Bitget Order Connector implementation
    - [ ] Create Bitget Order Connector integration example
    - [ ] Send small order to Bitget using limit price X% away from TOB
* [ ] Alloy Integration
    - [x] Calling contract methods
    - [x] Subscribtions to chain events
    - [ ] Check minimum balance of the wallet to prevent bots
* [ ] Collateral Router Bridges
    - [ ] Create Collateral Bridge between two EVM chains using Across
    - [ ] Create Collateral Bridge between two EVM wallets within same network 
    - [ ] Create Collateral Designation for Binance account
    - [ ] Send small amount of collateral from EVM to Binance
* [ ] Sell side implementation
    - [ ] Wait for index token collateral before selling assets
    - [ ] Obtain USD cash management details after filling index order
    - [ ] Confirm USD cash before withdrawal
    - [ ] Burn index token collateral
* [ ] Rebalance
    - [ ] Review rebalance code...
* [x] Implement additional limits
    - [x] Ensure individual orders in a batch have size at least minimum
    - [x] Ensure order rate is within limits
    - [x] Ensure total volley size across batches is not exceeded
* [ ] Additional Work for Binance Market Data Connector 
    - [ ] Test performance of many subscriptions
    - [ ] Investigate asset delisting
    - [ ] Investigate system to maintain the list of assets
    - [ ] Create Solver example with live market data
* [ ] Additional Work for Binance Order Sending Connector 
    - [x] Respect rate limits
    - [x] Respect price and quantity filters
    - [x] Handle private user data notifications
* [ ] Logging & Error handling
    - [x] Choose Rust package for logging (see `tracing` package)
    - [ ] Find out best way to handle runtime errors (logging, automated recovery, responding to user)
    - [ ] Explore logging mechanism (craft log messages, configure logging to file)
    - [x] Error handling for server events
* [ ] Reporting & On-Chain History Recording
    - [ ] Invastigate reportable events to store on-chain as proof of transaction
    - [ ] Design on-chain reporting mechanism
* [ ] Improve SBE Test Scenario
    - [ ] Convert SBE test scenario into a framework for scenario testing
    - [ ] Provide multiple test scenarios
    - [ ] Hone main scenario to produce human friendly numbers
* [ ] Improve Solver Strategy
    - [ ] Investigate behaviour of SimpleSolver in various scenarios
    - [ ] Adjust SimpleSolver liquiditiy fitting algoithm
    - [ ] Adjust SimpleSolver quoting algorithm
    - [ ] See if MatrixSolver would be beneficial
* [ ] Improve Index Order Engage
    - [ ] Investigate if we want to carry engagement into next updates
    - [ ] Investigate left-over collateral that stays engaged after minting
* [ ] Improve Collateral Routing
    - [ ] Remove closed lots
    - [ ] Investigate what happens when there is both a deposit for Buy trade, and a withdrawal for Sell
    - [ ] Multiple destinations
    - [ ] Create a relationship between destinations and Order Connector
    - [ ] Reuse collateral from cancelled order in the new order

#### Design & Performance

* [ ] Logging
    - [x] We need to review our logging, as at present messages and errors are logged to standard output.
    - [ ] We need to review log messages, and ensure they contain just right amount of information for us to understand what happened in the system, and not too much to avoid logs bloating and poor overall performance.
    - [x] We need to find logging mechanism that we want to use, and test it so that we know how it performs.
* [ ] Error handling
    - [ ] We need to review our error handling, as we are currently bailing out in case of an error, and we might be leaving some state inconsistent (especially if arithmetic error happens in the middle of calculations).
* [ ] Multi-Threaded Performance
    - [x] Evaluate performance & behaviour of demo application running separate threads
    for quote backend, order backend, solver, and market data.
    - [ ] We need to review design of queues and events in context of performance and
    maintainability, i.e. we use Mutex and RwLock from parking-lot, and then
    standard VecDeque and HashMap. The VecDeque gives us ability to efficiently
    drain number of elements from the front using some predicate.  The parking-lot
    implementations of Mutex and RwLock use atomics, and acquiring locks is
    relatively cheap. This approach is good, and should give decent performance,
    however under contention those queues will be accessed in serialized fashion,
    i.e. only one thread can read or write from them. We were thinking of using
    channels, or concurrent queue (cross-beam SegQueue). There are also
    implementations of concurrent hash-maps that we could explore.
* [x] Web Framework Integration
    - [x] Decision to use Axum for web server (REST and WSS)
* [ ] Disaster recovery plan
    - We need to understand our system well, make sure logging is 

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

## Phase 2. Integration

### Goal

The goal is to have working demoable MVP where we can send Index Orders over FIX, and we want to
see Solver acquiring assets from Binance, and reporting mint.

The resulting MVP should be able to demonstrate following:
- User can send Index Order over FIX/Json to Solver service
- Solver receives latest asset prices and order book updates from Binance
- Solver creates a batch of Asset Orders, and sends them out to Binance
- Solver responds to filled Asset Orders by assigning lots to Index Orders
- Solver sends back to the user Fill Reports and eventually Mint Invoice

### Binance Market Data

The goal is to integrate market data from Binance, and observe system behavior and accuracy dependant on changing market conditions.

We will use Binance SDK provided directly by Binance to receive market data and track asset prices, and build books.

### Binance Orders

The goal is to integrate order sending with Binance, and to confirm that orders are sent, they don't exceed limits, they get
executed, we track them correctly all the way up until minting.

We will use Binance SDK to log onto Binance account, obtain exchange information about traded assets, and to send orders.
The order sender will respect order rate limits and will apply price & quantity filters to orders before sending to Binance.

### FIX/Json over Web Sockets

The goal is to have fully functional FIX server capable of receiving NewOrder, CancelOrder etc messages.

We will use Axum web framework to provide web socket server, and on top of that we will privde FIX/Json
protocol to the users. The sever will have a plugin, which will translate from FIX/Json messages into
application specific events and from application specific reponses into FIX/Json responses. Plugin will
be provided by application, i.e. Solver (Index Maker).

### EVM Network

The goal is to have fully functional chain integration so that we can receive on-chain events and invoke on-chain smart-contract methods.

We will use Alloy framework to provide interop with EVM chains, and on top of that we will provide
Chain Connector implementation that will emit application events, and will provide application level methods, that will translate
into chain specific smart-contract method calls. 

### Collateral Routing

The goal is to have fully functional collateral routing implemented, so that collateral can be routed
from Arbitrum to Binance via Base. The routing between Arbitrum and Base is to be done via Across bridge, while
the routing from Base to Binance should happend between two EVM wallets. The wallet address for Binance account
need to be obtained via Binance API call.

## Phase X. Future Development

After delivering working system, which is capable of accepting Index Orders, and sending order batches to Binance, we will
look into robustness of the solution, performance, scalling, fixing bugs and adding more features.





