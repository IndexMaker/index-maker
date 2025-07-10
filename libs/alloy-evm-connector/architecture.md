# Alloy-EVM-Connector Architecture

## Architecture Diagram

```mermaid
graph TD
    %% Main Components
    API[EvmConnector<br/>Public API] 
    ARB[Arbiter<br/>Coordinator]
    OPS[ChainOperations<br/>Manager]
    
    %% Workers
    W1[ChainOperation<br/>Worker 1<br/>Chain ID: 1]
    W2[ChainOperation<br/>Worker 2<br/>Chain ID: 42161]
    W3[ChainOperation<br/>Worker 3<br/>Chain ID: 8453]
    WN[ChainOperation<br/>Worker N<br/>Chain ID: N]
    
    %% Supporting Components
    OBS[Observer<br/>Notifications]
    CMD[ChainCommands<br/>& Requests]
    
    %% Data Flow
    API -->|ChainOperationRequest| ARB
    ARB -->|Route Request| OPS
    OPS -->|Distribute| W1
    OPS -->|Distribute| W2
    OPS -->|Distribute| W3
    OPS -->|Distribute| WN
    
    %% Response Flow
    W1 -->|ChainOperationResult| OPS
    W2 -->|ChainOperationResult| OPS
    W3 -->|ChainOperationResult| OPS
    WN -->|ChainOperationResult| OPS
    OPS -->|Aggregate Results| ARB
    ARB -->|Response| API
    
    %% Event Publishing
    W1 -.->|BlockchainEvents| OBS
    W2 -.->|BlockchainEvents| OBS
    W3 -.->|BlockchainEvents| OBS
    WN -.->|BlockchainEvents| OBS
    OBS -.->|ChainNotification| API
    
    %% Command Flow
    CMD -->|Commands| API
    
    %% Styling
    classDef publicAPI fill:#e1f5fe
    classDef coordinator fill:#f3e5f5
    classDef manager fill:#e8f5e8
    classDef worker fill:#fff3e0
    classDef support fill:#fafafa
    
    class API publicAPI
    class ARB coordinator
    class OPS manager
    class W1,W2,W3,WN worker
    class OBS,CMD support
```

## Component Details

### Public API Layer
- **EvmConnector**: Main interface for blockchain operations
  - Connects to multiple chains
  - Manages chain lifecycle (connect/disconnect)
  - Implements ChainConnector trait
  - Publishes blockchain events via Observer pattern

### Coordination Layer
- **Arbiter**: Central async coordinator
  - Three core functions: `new()`, `start()`, `stop()`
  - Routes requests between API and workers
  - Manages resource lifecycle with AsyncLoop
  - Handles graceful shutdown

### Worker Management Layer
- **ChainOperations**: Worker pool manager
  - Manages multiple ChainOperation workers
  - Routes commands to appropriate chain workers
  - Aggregates results from workers
  - Handles worker lifecycle

### Worker Layer
- **ChainOperation**: Individual blockchain workers
  - One worker per blockchain (Ethereum, Arbitrum, Base, etc.)
  - Handles custody operations (custodyToConnector, callConnector)
  - Manages blockchain provider and wallet
  - Executes smart contract interactions

## Key Operations

### Custody Operations
- **CustodyToConnector**: Transfer funds from custody to connector
- **CallConnector**: Execute connector operations (e.g., Across deposits)
- **SetupCustody**: Initialize custody with tokens
- **ApproveToken**: Approve token spending

### Cross-Chain Operations
- **Across Protocol Integration**: Cross-chain bridge operations
- **Merkle Proof Generation**: CAHelper for secure custody operations
- **Multi-Chain Coordination**: Simultaneous operations across chains

## Data Flow

1. **Request Flow**: EvmConnector → Arbiter → ChainOperations → ChainOperation
2. **Response Flow**: ChainOperation → ChainOperations → Arbiter → EvmConnector
3. **Event Flow**: ChainOperation → Observer → EvmConnector → Main System

## Channel Communication

```mermaid
sequenceDiagram
    participant EC as EvmConnector
    participant A as Arbiter
    participant CO as ChainOperations
    participant COP as ChainOperation
    
    EC->>A: ChainOperationRequest
    A->>CO: Route Request
    CO->>COP: Execute Command
    COP->>COP: Blockchain Operation
    COP->>CO: Operation Result
    CO->>A: Aggregate Result
    A->>EC: Final Response
    
    Note over COP,EC: Async communication via<br/>tokio::sync::mpsc channels
```

## Architecture Benefits

- **Scalability**: Easy to add new blockchain networks
- **Reliability**: Isolated workers prevent cross-chain failures
- **Maintainability**: Clear separation of concerns
- **Async Performance**: Non-blocking operations across all layers
- **Consistent Pattern**: Follows proven binance module architecture