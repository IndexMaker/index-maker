#!/bin/bash


kill -9 `ps uax | grep "anvil" | grep -v grep | grep -v run | awk '{print $2}'`

RUST_LOG=info,node=warn cargo run -p anvil_provisioner
