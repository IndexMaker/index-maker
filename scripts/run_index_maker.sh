#!/bin/bash

: "${SERVER_PRIVATE_KEY:?Error: SERVER_PRIVATE_KEY environment variable not set.}"
: "${INDEX_MAKER_PRIVATE_KEY:?Error: INDEX_MAKER_PRIVATE_KEY environment variable not set.}"
: "${CUSTODY_AUTHORITY_PRIVATE_KEY:?Error: CUSTODY_AUTHORITY_PRIVATE_KEY environment variable not set.}"

RUST_BACKTRACE=1 RUST_LOG=info,binance_sdk=off,hyper_util=off,binance=warn,axum=warn,market_data:warn cargo run -- -s fix-server 0
@
