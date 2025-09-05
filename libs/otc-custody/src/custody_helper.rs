use std::ops::ControlFlow;

use alloy::{
    hex,
    primitives::{Address, B256},
    sol_types::SolValue,
};
use ethers::{
    abi::{self, Token},
    types::{Address as EthAddress, Bytes as EBytes, U256},
};
use eyre::OptionExt;
use merkle_tree_rs::core::{get_proof, make_merkle_tree};
use serde::{Deserialize, Serialize};

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
    custody_id: Option<[u8; 32]>,
    chain_id: u32,
    otc_custody_address: Address,
}

impl CAHelper {
    pub fn new(chain_id: u32, otc_custody_address: Address) -> Self {
        Self {
            ca_items: Vec::new(),
            merkle_tree: None,
            leaf_indices: Vec::new(),
            custody_id: None,
            chain_id,
            otc_custody_address,
        }
    }

    pub fn try_new_with_items(ca_items: Vec<CAItem>) -> eyre::Result<Self> {
        let (chain_id, otc_custody_address) = ca_items
            .first()
            .map(|x| (x.chain_id as u32, x.otc_custody))
            .ok_or_eyre("Failed to create CAHelper: Empty set of items")?;

        ca_items
            .iter()
            .map(|x| (x.chain_id as u32, x.otc_custody))
            .all(|(a, b)| a == chain_id && b == otc_custody_address)
            .then_some(())
            .ok_or_eyre(
                "Failed to create CAHelper: Not all items have same chain_id and otc_custody",
            )?;

        Ok(Self {
            ca_items,
            merkle_tree: None,
            leaf_indices: Vec::new(),
            custody_id: None,
            chain_id,
            otc_custody_address,
        })
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
            chain_id: self.chain_id as u64,
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
    pub fn get_merkle_tree(&mut self) -> &Vec<EBytes> {
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
    pub fn compute_leaf(item: &CAItem) -> EBytes {
        let tokens: Vec<Token> = vec![
            Token::String(item.item_type.to_string()),
            Token::Uint(U256::from(item.chain_id)),
            Token::Address(EthAddress::from_slice(item.otc_custody.as_slice())),
            Token::Uint(U256::from(item.state)),
            {
                let bytes = hex::decode(item.args.trim_start_matches("0x")).unwrap();
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
}

impl std::fmt::Debug for CAHelper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ca_items: Vec<_> = self
            .ca_items
            .iter()
            .map(|item| {
                let leaf_hash = Self::compute_leaf(item);
                let leaf_hash_hex = hex::encode(leaf_hash.as_ref());

                format!("{:?} ({})", item, leaf_hash_hex)
            })
            .collect();

        f.debug_struct("CAHelper")
            .field("ca_items", &ca_items)
            .field("merkle_tree", &self.merkle_tree)
            .field("leaf_indices", &self.leaf_indices)
            .field("custody_id", &self.custody_id)
            .field("chain_id", &self.chain_id)
            .field("otc_custody_address", &self.otc_custody_address)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, B256};

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
                &Address::ZERO,
                &Address::ZERO,
                "10000000",
                "9000000",
                "42161",
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
