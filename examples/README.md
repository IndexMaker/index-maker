# Examples

## EVM Connector

We showcase EVM integration tech, which consists of EVM connector, collateral bridge, and events.

### How to run

Start Anvil Orchestrator:
```
    FORK_URL=https://arb1.lava.build cargo run -p anvil_orchestrator
```

Take a note of DepositEmitter shown as log line:
```
2025-08-19T09:30:20.567212Z  INFO DepositEmitter deployed at: 0x405c38a4939fce0bce49e21c717c8c093d95618b
```

Open up in a web browser:
```
[path-to-index-maker]/apps/test_client/client.html
```

Put noted above address into "DepositEmitter Address" field. The "address" of the "Request JSON" needs to be
one of the hard-coded addresses (check the code) to enable collateral routing, and otherwise deposit will
be seen, but not routed.

Run example:
```
RUST_LOG=trace,alloy=info,hyper=info cargo run --example evm-connector
```