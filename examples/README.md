# Examples

## EVM Connector

We showcase EVM integration tech, which consists of EVM connector, collateral bridge, and events.

### How to run

Start Anvil Orchestrator:
```
    FORK_URL=https://arb1.lava.build cargo run -p anvil_orchestrator
````

Open up in a web browser:
```
[path-to-index-maker]/apps/test_client/client.html
```

Run example:
```
RUST_LOG=trace,alloy=info,hyper=info cargo run --example evm-connector
```