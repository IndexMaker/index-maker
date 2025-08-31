use crate::{custody_helper::CAItem, index::index_helper::IndexDeployData};
use alloy::primitives::{Address, B256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDeploymentBuilderData {
    pub chain_id: u32,
    pub index_factory_address: Address,
    pub custody_address: Address,
    pub trade_route: Address,
    pub withdraw_route: Address,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDeploymentData {
    pub index_deploy_data: IndexDeployData,
    pub index_factory_address: Address,
    pub custody_address: Address,
    pub trade_route: Address,
    pub withdraw_route: Address,
    pub custody_id: B256,
    pub ca_items: Vec<CAItem>,
    pub index_deploy_proof: Vec<B256>,
    pub trade_route_proof: Vec<B256>,
    pub withdraw_route_proof: Vec<B256>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInstanceData {
    pub index_address: Address,
    pub deployment_data: IndexDeploymentData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDeployerData {
    #[serde(flatten)]
    pub deployment_builder_data: IndexDeploymentBuilderData,
    pub indexes: Vec<IndexDeployData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMakerData {
    #[serde(flatten)]
    pub deployment_builder_data: IndexDeploymentBuilderData,
    pub indexes: Vec<IndexInstanceData>,
}