# Index Maker

## Roadmap

### Phase 1. (Complete) Mocked I/O (FIX / Chain/ Market Data)

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

### Phase 2. (In-Progress) Integration

The goal is to have working demoable MVP where we can send Index Orders over FIX, and we want to
see Solver acquiring assets from Binance, and reporting mint.

The resulting MVP should be able to demonstrate following:
- User can send Index Order over FIX/Json to Solver service
- Solver receives latest asset prices and order book updates from Binance
- Solver creates a batch of Asset Orders, and sends them out to Binance
- Solver responds to filled Asset Orders by assigning lots to Index Orders
- Solver sends back to the user Fill Reports and eventually Mint Invoice

#### Binance Market Data

The goal is to integrate market data from Binance, and observe system behavior and accuracy dependant on changing market conditions.

We will use Binance SDK provided directly by Binance to receive market data and track asset prices, and build books.

#### Binance Orders

The goal is to integrate order sending with Binance, and to confirm that orders are sent, they don't exceed limits, they get
executed, we track them correctly all the way up until minting.

We will use Binance SDK to log onto Binance account, obtain exchange information about traded assets, and to send orders.
The order sender will respect order rate limits and will apply price & quantity filters to orders before sending to Binance.

#### FIX/Json over Web Sockets

The goal is to have fully functional FIX server capable of receiving NewOrder, CancelOrder etc messages.

We will use Axum web framework to provide web socket server, and on top of that we will privde FIX/Json
protocol to the users. The sever will have a plugin, which will translate from FIX/Json messages into
application specific events and from application specific reponses into FIX/Json responses. Plugin will
be provided by application, i.e. Solver (Index Maker).

#### EVM Network

The goal is to have fully functional chain integration so that we can receive on-chain events and invoke on-chain smart-contract methods.

We will use Alloy framework to provide interop with EVM chains, and on top of that we will provide
Chain Connector implementation that will emit application events, and will provide application level methods, that will translate
into chain specific smart-contract method calls. 

#### Collateral Routing

The goal is to have fully functional collateral routing implemented, so that collateral can be routed
from Arbitrum to Binance via Base. The routing between Arbitrum and Base is to be done via Across bridge, while
the routing from Base to Binance should happend between two EVM wallets. The wallet address for Binance account
need to be obtained via Binance API call.

### Phase X. Future Development

After delivering working system, which is capable of accepting Index Orders, and sending order batches to Binance, we will
look into robustness of the solution, performance, scalling, fixing bugs and adding more features.





