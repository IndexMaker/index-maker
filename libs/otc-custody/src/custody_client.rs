use std::sync::Arc;

use alloy::{
    primitives::{Address, B256, U256},
    providers::DynProvider,
    rpc::types::TransactionReceipt,
};
use async_trait::async_trait;

#[async_trait]
pub trait CustodyClientMethods {
    fn get_custody_id(&self) -> B256;
    fn get_custody_address(&self) -> &Address;
    fn get_collateral_token_address(&self) -> &Address;
    fn get_collateral_token_precision(&self) -> u8;

    async fn get_custody_owner(&self, provider: &DynProvider) -> eyre::Result<Address>;

    async fn route_collateral_to_from(
        &self,
        providers: &[&DynProvider],
        from_address: &Address,
        to_address: &Address,
        token_address: &Address,
        amount: U256,
    ) -> eyre::Result<TransactionReceipt>;
}

#[derive(Clone)]
pub struct CustodyClient(Arc<dyn CustodyClientMethods + Send + Sync + 'static>);

impl CustodyClient {
    pub fn new(inner: Arc<dyn CustodyClientMethods + Send + Sync + 'static>) -> Self {
        Self(inner)
    }

    pub fn get_custody_id(&self) -> B256 {
        self.0.get_custody_id()
    }

    pub fn get_custody_address(&self) -> &Address {
        self.0.get_custody_address()
    }

    pub fn get_collateral_token_address(&self) -> &Address {
        self.0.get_collateral_token_address()
    }

    pub fn get_collateral_token_precision(&self) -> u8 {
        self.0.get_collateral_token_precision()
    }

    pub async fn get_custody_owner(&self, provider: &DynProvider) -> eyre::Result<Address> {
        self.0.get_custody_owner(provider).await
    }

    pub async fn route_collateral_to_from(
        &self,
        providers: &[&DynProvider],
        from_address: &Address,
        to_address: &Address,
        token_address: &Address,
        amount: U256,
    ) -> eyre::Result<TransactionReceipt> {
        self.0
            .route_collateral_to_from(providers, from_address, to_address, token_address, amount)
            .await
    }
}
