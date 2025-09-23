#!/bin/bash

: "${SERVER_PRIVATE_KEY:?Error: SERVER_PRIVATE_KEY environment variable not set.}"
: "${INDEX_MAKER_PRIVATE_KEY:?Error: INDEX_MAKER_PRIVATE_KEY environment variable not set.}"
: "${CUSTODY_AUTHORITY_PRIVATE_KEY:?Error: CUSTODY_AUTHORITY_PRIVATE_KEY environment variable not set.}"

cargo run -- --rpc-urls wss://base-rpc.publicnode.com,https://1rpc.io/base,https://mainnet.base.org --query-bind-address 127.0.0.1:8080 -s --config-path ./configs/dev fix-server 0

