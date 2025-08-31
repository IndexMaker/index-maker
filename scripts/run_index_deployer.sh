#!/bin/bash

RUST_LOG=info cargo run -p index_deployer -- $@
