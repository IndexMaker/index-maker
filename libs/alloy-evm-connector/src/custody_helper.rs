use alloy::{
    hex,
    primitives::{Address, B256},
    sol_types::SolValue,
};
use merkle_tree_rs::standard::{LeafType, StandardMerkleTree};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    merkle_tree: Option<StandardMerkleTree>,
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
            args_to_types,
            custody_id: None,
            chain_id,
            otc_custody_address,
        }
    }

    fn encode_args(&self, item_type: &CAItemType, args: &[(&str, &[u8])]) -> String {
        if let Some(param_types) = self.args_to_types.get(item_type) {
            if param_types.is_empty() {
                return "0x".to_string();
            }

            // Convert args to a tuple of values
            let values: Vec<&[u8]> = args.iter().map(|(_, value)| *value).collect();

            // Use alloy's abi_encode_params to encode the values
            let encoded = SolValue::abi_encode_params(&values);
            format!("0x{}", hex::encode(encoded))
        } else {
            "0x".to_string()
        }
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
        if item_type == CAItemType::CallConnector {
            // Handle special case for callConnector where callData might be an object
            if let Some((_, call_data)) = args.iter().find(|(name, _)| *name == "callData") {
                if let Ok(call_data_str) = std::str::from_utf8(call_data) {
                    if let Ok(call_data_obj) =
                        serde_json::from_str::<serde_json::Value>(call_data_str)
                    {
                        if let (Some(func_type), Some(func_args)) = (
                            call_data_obj.get("type").and_then(|t| t.as_str()),
                            call_data_obj.get("args").and_then(|a| a.as_array()),
                        ) {
                            let func_args_bytes: Vec<&[u8]> = func_args
                                .iter()
                                .filter_map(|arg| arg.as_str().map(|s| s.as_bytes()))
                                .collect();
                            let encoded_call_data =
                                self.encode_calldata(func_type, &func_args_bytes);
                            let encoded_bytes = encoded_call_data.as_bytes().to_vec();
                            for (name, value) in args {
                                if *name == "callData" {
                                    values.push((*name, encoded_bytes.clone()));
                                } else {
                                    values.push((*name, value.to_vec()));
                                }
                            }
                        }
                    }
                }
            }
        } else {
            for (name, value) in args {
                values.push((*name, value.to_vec()));
            }
        }
        let new_args: Vec<(&str, &[u8])> = values
            .iter()
            .map(|(name, value)| (*name, value.as_slice()))
            .collect();

        let encoded_args = if item_type == CAItemType::CallConnector {
            self.encode_args(&item_type, &new_args)
        } else {
            self.encode_args(&item_type, &new_args)
        };

        let item = CAItem {
            item_type,
            chain_id: self.chain_id,
            otc_custody: self.otc_custody_address,
            state,
            args: encoded_args,
            party,
        };

        self.ca_items.push(item);
        self.merkle_tree = None; // Invalidate the tree to be reconstructed at next call

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

    fn get_merkle_tree(&mut self) -> &StandardMerkleTree {
        if self.merkle_tree.is_none() {
            let values: Vec<Vec<String>> = self
                .ca_items
                .iter()
                .map(|item| {
                    vec![
                        item.item_type.to_string(),
                        item.chain_id.to_string(),
                        item.otc_custody.to_string(),
                        item.state.to_string(),
                        hex::encode(&item.args),
                        item.party.parity.to_string(),
                        hex::encode(item.party.x),
                    ]
                })
                .collect();

            let types = [
                "string".to_string(),  // entry type
                "uint256".to_string(), // chainId
                "address".to_string(), // otcCustody
                "uint".to_string(),    // state
                "string".to_string(),  // abi.encode(args)
                "uint".to_string(),    // party.parity
                "string".to_string(),  // party.x
            ];

            self.merkle_tree = Some(StandardMerkleTree::of(values, &types));
        }
        self.merkle_tree.as_ref().unwrap()
    }

    pub fn get_custody_id(&mut self) -> [u8; 32] {
        if self.custody_id.is_none() {
            self.custody_id = Some(self.get_ca_root());
        }
        self.custody_id.clone().unwrap()
    }

    pub fn get_ca_root(&mut self) -> [u8; 32] {
        let tree = self.get_merkle_tree();
        let root = tree.root();

        // The root is returned as a hex string, so we need to decode it
        let root_str = root.as_str();
        let root_bytes = hex::decode(root_str.strip_prefix("0x").unwrap_or(root_str))
            .expect("Failed to decode merkle root");

        let mut custody_id = [0u8; 32];
        custody_id.copy_from_slice(&root_bytes);
        custody_id
    }

    pub fn get_merkle_proof(&mut self, index: usize) -> Vec<[u8; 32]> {
        if self.ca_items.len() == 1 {
            return vec![];
        }
        if index >= self.ca_items.len() {
            panic!(
                "Invalid index: {}. Valid range is 0-{}",
                index,
                self.ca_items.len() - 1
            );
        }

        let tree = self.get_merkle_tree();
        let proof = tree.get_proof(LeafType::Number(index));
        proof
            .iter()
            .map(|p| {
                let proof_str = p.as_str();
                let proof_bytes = hex::decode(proof_str.strip_prefix("0x").unwrap_or(proof_str))
                    .expect("Failed to decode merkle proof");
                let mut proof_array = [0u8; 32];
                proof_array.copy_from_slice(&proof_bytes);
                proof_array
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.ca_items.clear();
        self.merkle_tree = None;
        self.custody_id = None;
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
        println!("Custody ID (merkle root): {:?}", custody_id);

        // Get all CA items and their proofs
        let all_items = ca_helper.get_ca_items();
        let root = ca_helper.get_ca_root();
        let tree = ca_helper.get_merkle_tree();
        let proofs: Vec<_> = (0..all_items.len())
            .map(|i| ca_helper.get_merkle_proof(i))
            .collect();

        let all_actions_with_proofs: Vec<_> = (0..all_items.len())
            .map(|i| (i, all_items[i].clone(), proofs[i].clone()))
            .collect();
        println!("Total actions: {}", all_actions_with_proofs.len());
        println!("Proofs: {:?}", proofs.clone());

        // Verify the merkle proofs
        for (index, item, proof) in all_actions_with_proofs {
            // Verify the proof is valid
            assert!(
                !proof.is_empty() || all_items.len() == 1,
                "Proof should be empty only when there's a single item"
            );
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
}
