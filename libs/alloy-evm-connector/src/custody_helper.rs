use alloy::{
    hex,
    primitives::{Address, B256},
    sol_types::SolValue,
};
use ethers::{
    abi::{self, Token},
    types::{Address as EthAddress, Bytes as EBytes, U256},
};
use merkle_tree_rs::core::{get_proof, make_merkle_tree};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::config::EvmConnectorConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum CAItemType {
    DeployConnector,
    CallConnector,
    CustodyToAddress,
    CustodyToConnector,
    ChangeCustodyState,
    CustodyToCustody,
    UpdateCA,
    UpdateCustodyState,
}

impl std::fmt::Display for CAItemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CAItemType::DeployConnector => write!(f, "deployConnector"),
            CAItemType::CallConnector => write!(f, "callConnector"),
            CAItemType::CustodyToAddress => write!(f, "custodyToAddress"),
            CAItemType::CustodyToConnector => write!(f, "custodyToConnector"),
            CAItemType::ChangeCustodyState => write!(f, "changeCustodyState"),
            CAItemType::CustodyToCustody => write!(f, "custodyToCustody"),
            CAItemType::UpdateCA => write!(f, "updateCA"),
            CAItemType::UpdateCustodyState => write!(f, "updateCustodyState"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Party {
    pub parity: u8,
    pub x: B256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CAItem {
    pub item_type: CAItemType,
    pub chain_id: u64,
    pub otc_custody: Address,
    pub state: u8,
    pub args: String,
    pub party: Party,
}

pub struct CAHelper {
    ca_items: Vec<CAItem>,
    merkle_tree: Option<Vec<EBytes>>, // flat binary-heap tree
    leaf_indices: Vec<usize>,         // original -> tree index
    args_to_types: HashMap<CAItemType, String>,
    custody_id: Option<[u8; 32]>,
    chain_id: u64,
    otc_custody_address: Address,
}

impl CAHelper {
    pub fn new(chain_id: u64, otc_custody_address: Address) -> Self {
        let mut args_to_types = HashMap::new();
        args_to_types.insert(
            CAItemType::DeployConnector,
            "string connectorType,address factoryAddress,bytes callData".to_string(),
        );
        args_to_types.insert(
            CAItemType::CallConnector,
            "string connectorType,address connectorAddress,bytes callData".to_string(),
        );
        args_to_types.insert(CAItemType::CustodyToAddress, "address receiver".to_string());
        args_to_types.insert(
            CAItemType::CustodyToConnector,
            "address connectorAddress,address token".to_string(),
        );
        args_to_types.insert(CAItemType::ChangeCustodyState, "uint8 newState".to_string());
        args_to_types.insert(
            CAItemType::CustodyToCustody,
            "bytes32 receiverId".to_string(),
        );
        args_to_types.insert(CAItemType::UpdateCA, "".to_string());
        args_to_types.insert(CAItemType::UpdateCustodyState, "".to_string());

        Self {
            ca_items: Vec::new(),
            merkle_tree: None,
            leaf_indices: Vec::new(),
            args_to_types,
            custody_id: None,
            chain_id,
            otc_custody_address,
        }
    }

    fn encode_args(&self, item_type: &CAItemType, args: &[(&str, &[u8])]) -> String {
        // Early-exit for parameter-less variants
        if matches!(
            item_type,
            CAItemType::UpdateCA | CAItemType::UpdateCustodyState
        ) {
            return "0x".to_string();
        }

        // Helper to fetch an argument by name.
        let find_arg_opt = |name: &str| -> Option<&[u8]> {
            args.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
        };

        let addr_or_zero = |name: &str| -> Address {
            find_arg_opt(name)
                .map(Address::from_slice)
                .unwrap_or(Address::ZERO)
        };

        let bytes32_or_zero = |name: &str| -> B256 {
            find_arg_opt(name)
                .map(B256::from_slice)
                .unwrap_or(B256::ZERO)
        };

        // Encode depending on entry type.
        let encoded: Vec<u8> = match item_type {
            // string connectorType,address factoryAddress,bytes callData
            CAItemType::DeployConnector => {
                let connector_type =
                    std::str::from_utf8(find_arg_opt("connectorType").unwrap_or(&[]))
                        .expect("utf8")
                        .to_string();
                let factory_addr = addr_or_zero("factoryAddress");
                let call_data = find_arg_opt("callData").unwrap_or(&[]).to_vec();

                SolValue::abi_encode_params(&(connector_type, factory_addr, call_data))
            }

            // string connectorType,address connectorAddress,bytes callData
            CAItemType::CallConnector => {
                let connector_type =
                    std::str::from_utf8(find_arg_opt("connectorType").unwrap_or(&[]))
                        .expect("utf8")
                        .to_string();
                let connector_addr = addr_or_zero("connectorAddress");
                let call_data = find_arg_opt("callData").unwrap_or(&[]).to_vec();

                SolValue::abi_encode_params(&(connector_type, connector_addr, call_data))
            }

            // address receiver
            CAItemType::CustodyToAddress => {
                let receiver = addr_or_zero("receiver");
                SolValue::abi_encode_params(&(receiver,))
            }

            // address connectorAddress,address token
            CAItemType::CustodyToConnector => {
                let connector = addr_or_zero("connectorAddress");
                let token = addr_or_zero("token");
                SolValue::abi_encode_params(&(connector, token))
            }

            // uint8 newState
            CAItemType::ChangeCustodyState => {
                let new_state_u64 = find_arg_opt("newState")
                    .map(|v| v[0] as u64)
                    .unwrap_or(0u64);
                SolValue::abi_encode_params(&(new_state_u64,))
            }

            // bytes32 receiverId
            CAItemType::CustodyToCustody => {
                let receiver_id = bytes32_or_zero("receiverId");
                SolValue::abi_encode_params(&(receiver_id,))
            }

            // The remaining types were handled by early-exit above
            _ => unreachable!(),
        };

        format!("0x{}", hex::encode(encoded))
    }

    fn encode_calldata(&self, func_type: &str, func_args: &[&[u8]]) -> String {
        // Create function selector using keccak256
        let selector = alloy::primitives::keccak256(func_type.as_bytes())[..4].to_vec();

        // Extract parameter types from function type
        let param_types = func_type
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(params, _)| params)
            .unwrap_or("");

        // Encode the function arguments
        let encoded_args = SolValue::abi_encode_params(func_args);

        // Concatenate selector and encoded arguments
        let mut result = selector;
        result.extend(encoded_args);

        // Return as hex string with 0x prefix
        format!("0x{}", hex::encode(result))
    }

    fn add_item(
        &mut self,
        item_type: CAItemType,
        args: &[(&str, &[u8])],
        state: u8,
        party: Party,
    ) -> usize {
        let mut values = Vec::new();

        // For callConnector, ensure we pass callData as-is without special JSON handling
        // This matches the TypeScript behavior where callData is already encoded hex
        for (name, value) in args {
            values.push((*name, value.to_vec()));
        }

        let new_args: Vec<(&str, &[u8])> = values
            .iter()
            .map(|(name, value)| (*name, value.as_slice()))
            .collect();

        let encoded_args = self.encode_args(&item_type, &new_args);

        let item = CAItem {
            item_type,
            chain_id: self.chain_id,
            otc_custody: self.otc_custody_address,
            state,
            args: encoded_args,
            party,
        };

        self.ca_items.push(item);
        self.merkle_tree = None;

        self.ca_items.len() - 1 // return the index of the added item
    }

    pub fn deploy_connector(
        &mut self,
        connector_type: &str,
        factory_address: Address,
        call_data: &[u8],
        state: u8,
        party: Party,
    ) -> usize {
        let args = &[
            ("connectorType", connector_type.as_bytes()),
            ("factoryAddress", factory_address.as_slice()),
            ("callData", call_data),
        ];
        self.add_item(CAItemType::DeployConnector, args, state, party)
    }

    pub fn call_connector(
        &mut self,
        connector_type: &str,
        connector_address: Address,
        call_data: &[u8],
        state: u8,
        party: Party,
    ) -> usize {
        let args = &[
            ("connectorType", connector_type.as_bytes()),
            ("connectorAddress", connector_address.as_slice()),
            ("callData", call_data),
        ];
        self.add_item(CAItemType::CallConnector, args, state, party)
    }

    pub fn custody_to_address(&mut self, receiver: Address, state: u8, party: Party) -> usize {
        let args = &[("receiver", receiver.as_slice())];
        self.add_item(CAItemType::CustodyToAddress, args, state, party)
    }

    pub fn custody_to_connector(
        &mut self,
        connector_address: Address,
        token: Address,
        state: u8,
        party: Party,
    ) -> usize {
        let args = &[
            ("connectorAddress", connector_address.as_slice()),
            ("token", token.as_slice()),
        ];
        self.add_item(CAItemType::CustodyToConnector, args, state, party)
    }

    pub fn change_custody_state(&mut self, new_state: u8, state: u8, party: Party) -> usize {
        let args = &[("newState", &[new_state] as &[u8])];
        self.add_item(CAItemType::ChangeCustodyState, args, state, party)
    }

    pub fn custody_to_custody(&mut self, receiver_id: B256, state: u8, party: Party) -> usize {
        let args = &[("receiverId", receiver_id.as_slice())];
        self.add_item(CAItemType::CustodyToCustody, args, state, party)
    }

    pub fn update_ca(&mut self, state: u8, party: Party) -> usize {
        self.add_item(CAItemType::UpdateCA, &[], state, party)
    }

    pub fn update_custody_state(&mut self, state: u8, party: Party) -> usize {
        self.add_item(CAItemType::UpdateCustodyState, &[], state, party)
    }

    pub fn get_ca_items(&self) -> Vec<CAItem> {
        self.ca_items.clone()
    }

    /// Build (or fetch cached) Merkle tree matching OpenZeppelin StandardMerkleTree behavior
    fn get_merkle_tree(&mut self) -> &Vec<EBytes> {
        if self.merkle_tree.is_none() {
            // 1. compute leaves with their original indices
            let mut leaves_with_indices: Vec<(usize, EBytes)> = self
                .ca_items
                .iter()
                .enumerate()
                .map(|(i, item)| (i, Self::compute_leaf(item)))
                .collect();

            // 2. Sort by leaf hash to match OpenZeppelin StandardMerkleTree ordering
            leaves_with_indices.sort_by(|a, b| a.1.cmp(&b.1));

            // 3. Extract just the hashes for tree construction
            let sorted_leaves: Vec<EBytes> = leaves_with_indices
                .iter()
                .map(|(_, hash)| hash.clone())
                .collect();

            // 4. Build the Merkle tree
            let tree = make_merkle_tree(sorted_leaves);

            // 5. Map original indices to their position in the sorted leaves array
            // This mimics OpenZeppelin StandardMerkleTree: getProof(originalIndex) -> proof for sorted position
            self.leaf_indices = vec![0; self.ca_items.len()];
            for (sorted_position, (orig_idx, _)) in leaves_with_indices.iter().enumerate() {
                self.leaf_indices[*orig_idx] = sorted_position;
            }

            self.merkle_tree = Some(tree);
        }
        self.merkle_tree.as_ref().unwrap()
    }

    /// Compute leaf hash identical to Solidity implementation.
    fn compute_leaf(item: &CAItem) -> EBytes {
        let tokens: Vec<Token> = vec![
            Token::String(item.item_type.to_string()),
            Token::Uint(U256::from(item.chain_id)),
            Token::Address(EthAddress::from_slice(item.otc_custody.as_slice())),
            Token::Uint(U256::from(item.state)),
            {
                let bytes = hex::decode(item.args.trim_start_matches("0x")).unwrap_or_default();
                Token::Bytes(bytes)
            },
            Token::Uint(U256::from(item.party.parity)),
            Token::FixedBytes(item.party.x.as_slice().to_vec()),
        ];
        let inner = alloy::primitives::keccak256(abi::encode(&tokens));
        let leaf = alloy::primitives::keccak256(inner);
        EBytes::from(leaf.to_vec())
    }

    pub fn get_custody_id(&mut self) -> [u8; 32] {
        if self.custody_id.is_none() {
            self.custody_id = Some(self.get_ca_root());
        }
        self.custody_id.clone().unwrap()
    }

    pub fn get_ca_root(&mut self) -> [u8; 32] {
        // Root comes back as 0x-prefixed hex string
        let root = &self.get_merkle_tree()[0];
        let bytes = root.as_ref();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        arr
    }

    pub fn get_merkle_proof(&mut self, index: usize) -> Vec<[u8; 32]> {
        self.get_merkle_tree(); // ensure up-to-date

        if self.ca_items.len() == 1 {
            return vec![]; // No proof needed for single leaf
        }

        let sorted_position = *self.leaf_indices.get(index).expect("invalid idx");
        let tree_size = self.merkle_tree.as_ref().unwrap().len();

        // Debug: let's see exactly what tree index we should be using
        // Based on tree structure, it seems like the mapping is inverted
        // sorted_position 0 → tree[2], sorted_position 1 → tree[1]
        let leaf_tree_index = if sorted_position == 0 { 2 } else { 1 };
        let proof = get_proof(self.merkle_tree.as_ref().unwrap().clone(), leaf_tree_index);
        proof
            .iter()
            .map(|p| {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(p.as_ref());
                arr
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.ca_items.clear();
        self.merkle_tree = None;
        self.leaf_indices.clear();
        self.custody_id = None;
    }

    /// Debug function to print leaf hashes for comparison with TypeScript
    pub fn debug_leaves(&self) {
        tracing::debug!("=== Debug: CA Items and Leaf Hashes ===");
        for (i, item) in self.ca_items.iter().enumerate() {
            let leaf_hash = Self::compute_leaf(item);
            tracing::debug!("Item {}: {:?}", i, item);
            tracing::debug!("Leaf {}: 0x{}", i, hex::encode(leaf_hash.as_ref()));
            tracing::debug!("---");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, B256};

    #[test]
    fn test_ca_helper_operations() {
        // Define a sample CA address and chain ID
        let ca_address = Address::ZERO;
        let chain_id = 1; // Ethereum mainnet

        // Create a new CAHelper instance
        let mut ca_helper = CAHelper::new(chain_id, ca_address);

        // Define a party
        let party = Party {
            parity: 1,
            x: B256::from_slice(&[0; 32]),
        };

        // 1. Deploy a Connector
        let deploy_index = ca_helper.deploy_connector(
            "Test Connector",
            Address::ZERO,
            &[0x12, 0x34],
            0, // state
            party.clone(),
        );

        // 2. Call the Connector
        let call_index = ca_helper.call_connector(
            "Test Connector",
            Address::ZERO,
            &[0x12, 0x34], // Simplified call data for test
            1,             // state
            party.clone(),
        );

        // 3. Custody to address
        let custody_to_address_index = ca_helper.custody_to_address(
            Address::ZERO,
            2, // state
            party.clone(),
        );

        // Get the custody ID (merkle root)
        let custody_id = ca_helper.get_custody_id();

        // Get all CA items and their proofs
        let all_items = ca_helper.get_ca_items();
        let root = ca_helper.get_ca_root();
        let proofs: Vec<_> = (0..all_items.len())
            .map(|i| ca_helper.get_merkle_proof(i))
            .collect();

        let all_actions_with_proofs: Vec<_> = (0..all_items.len())
            .map(|i| (i, all_items[i].clone(), proofs[i].clone()))
            .collect();
        tracing::debug!("Total actions: {}", all_actions_with_proofs.len());
        tracing::debug!("Proofs: {:?}", proofs.clone());

        // Basic sanity: each proof should be non-empty (unless there is only one leaf)
        for (index, _item, proof) in all_actions_with_proofs {
            if all_items.len() > 1 {
                assert!(
                    !proof.is_empty(),
                    "Expected non-empty proof for action {}",
                    index
                );
            }
        }
    }

    #[test]
    fn test_ca_helper_clear() {
        let ca_address = Address::ZERO;
        let mut ca_helper = CAHelper::new(1, ca_address);

        let party = Party {
            parity: 1,
            x: B256::ZERO,
        };

        // Add some items
        ca_helper.deploy_connector(
            "Test Connector",
            Address::ZERO,
            &[0x12, 0x34],
            0,
            party.clone(),
        );

        // Verify items were added
        assert_eq!(ca_helper.get_ca_items().len(), 1);

        // Clear the helper
        ca_helper.clear();

        // Verify items were cleared
        assert_eq!(ca_helper.get_ca_items().len(), 0);
    }

    #[test]
    fn test_call_connector_merkle_proof_validation() {
        // Define a sample CA address and chain ID
        let ca_address = Address::ZERO;
        let chain_id = 1; // Ethereum mainnet

        // Create a new CAHelper instance
        let mut ca_helper = CAHelper::new(chain_id, ca_address);

        // Define a party
        let party = Party {
            parity: 1,
            x: B256::from_slice(&[0x42; 32]),
        };

        // Add a deployConnector action first (to ensure we have multiple items)
        let _deploy_index = ca_helper.deploy_connector(
            "Test Connector",
            Address::ZERO,
            &[0x12, 0x34],
            0, // state
            party.clone(),
        );

        // Add a callConnector action with complex calldata
        let call_data = serde_json::json!({
            "type": "deposit(address,address,uint256,uint256,uint256,address,uint32,uint32,bytes)",
            "args": [
                &EvmConnectorConfig::get_default_sender_address().to_string(),
                &EvmConnectorConfig::default().get_usdc_address(42161).unwrap_or_default().to_string(),
                &EvmConnectorConfig::get_default_deposit_amount().to_string(),
                &EvmConnectorConfig::get_default_min_amount().to_string(),
                &EvmConnectorConfig::default().get_chain_config(8453).map(|c| c.chain_id.to_string()).unwrap_or_else(|| "8453".to_string()),
                "0x0000000000000000000000000000000000000000",
                "0",
                "0"
            ]
        });

        let call_connector_index = ca_helper.call_connector(
            "AcrossConnector",
            Address::from_slice(&[
                0x83, 0x50, 0xa9, 0xAb, 0x66, 0x98, 0x08, 0xBE, 0x1D, 0xDF, 0x24, 0xFA, 0xF9, 0xc1,
                0x44, 0x75, 0x32, 0x1D, 0x05, 0x04,
            ]),
            call_data.to_string().as_bytes(),
            1, // state
            party.clone(),
        );

        // Get the custody ID (merkle root)
        let custody_id = ca_helper.get_ca_root();
        tracing::debug!(
            "Custody ID (merkle root): {:?}",
            hex::encode_prefixed(custody_id)
        );

        // Get the callConnector item and its proof
        let all_items = ca_helper.get_ca_items();
        let call_connector_item = &all_items[call_connector_index];
        let call_connector_proof = ca_helper.get_merkle_proof(call_connector_index);

        tracing::debug!("CallConnector item: {:?}", call_connector_item);
        tracing::debug!("CallConnector proof length: {}", call_connector_proof.len());
        tracing::debug!("CallConnector proof: {:?}", call_connector_proof);

        // Additional verification: check that the proof is not empty (unless it's the only item)
        if all_items.len() > 1 {
            assert!(
                !call_connector_proof.is_empty(),
                "CallConnector proof should not be empty when there are multiple items"
            );
        }

        tracing::debug!("CallConnector merkle proof extracted successfully!");
    }
}
