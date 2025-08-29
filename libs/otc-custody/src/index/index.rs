use alloy::{
    primitives::{Address, Bytes, B256, U256},
    providers::{DynProvider, Provider},
    rpc::types::TransactionReceipt,
};
use async_trait::async_trait;
use eyre::eyre;

use crate::{
    connector_util::{call_connector_message, custody_to_address_message, encode_with_selector},
    contracts::{OTCCustody, VerificationData},
    custody_authority::CustodyAuthority,
    custody_client::{CustodyClient, CustodyClientMethods},
    index::index_helper::{
        encode_mint_call, IndexDeployData, OTC_INDEX_CONNECTOR_NAME,
        OTC_INDEX_CURATOR_WEIGHTS_CUSTODY_STATE, OTC_INDEX_MINT_CURATOR_STATE,
        OTC_INDEX_SOLVER_WEIGHTS_SET_STATE, OTC_TRADE_ROUTE_CUSTODY_STATE,
        OTC_WITHDRAW_ROUTE_CUSTODY_STATE,
    },
    util::{get_last_block_timestamp, pending_nonce},
};

pub struct IndexInstance {
    pub(crate) index_deploy_data: IndexDeployData,
    pub(crate) index_operator: CustodyAuthority,
    pub(crate) custody_address: Address,
    pub(crate) index_address: Address,
    pub(crate) trade_route: Address,
    pub(crate) withdraw_route: Address,
    pub(crate) custody_id: B256,
    pub(crate) trade_route_proof: Vec<B256>,
    pub(crate) withdraw_route_proof: Vec<B256>,
}

impl IndexInstance {
    fn get_verification_data(
        &self,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData> {
        self.index_operator.get_verification_data(
            self.custody_id,
            custody_state,
            timestamp,
            proof,
            message,
        )
    }

    pub fn get_index_address(&self) -> &Address {
        &self.index_address
    }

    pub fn get_symbol(&self) -> &str {
        &self.index_deploy_data.symbol
    }

    pub async fn set_currator_weights_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        weights: &Bytes,
        price: U256,
    ) -> eyre::Result<TransactionReceipt> {
        let timestamp = get_last_block_timestamp(&provider)
            .await
            .map_err(|err| eyre!("Failed to query current time: {:?}", err))?;

        let inner_message = encode_with_selector(
            "curatorUpdate(uint256,bytes,uint256)",
            timestamp,
            &weights,
            price,
        );

        let tail_call_data = Bytes::default();

        let message = call_connector_message(
            timestamp,
            self.custody_id,
            OTC_INDEX_CONNECTOR_NAME,
            self.index_address,
            &inner_message,
            &tail_call_data,
        );

        let proof = vec![]; // index is already in whitelisted so empty proof OK

        let verification_data = self.get_verification_data(
            OTC_INDEX_CURATOR_WEIGHTS_CUSTODY_STATE,
            timestamp,
            &proof,
            &message,
        )?;

        let otc = OTCCustody::new(self.custody_address, provider);
        let receipt = otc
            .callConnector(
                String::from(OTC_INDEX_CONNECTOR_NAME),
                self.index_address,
                message,
                tail_call_data,
                verification_data,
            )
            .from(from_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        if !receipt.status() {
            Err(eyre!("Failed to set curator weights: {:?}", receipt))?;
        }

        Ok(receipt)
    }

    pub async fn solver_weights_set_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        weights: &Bytes,
        price: U256,
    ) -> eyre::Result<TransactionReceipt> {
        let timestamp = get_last_block_timestamp(&provider)
            .await
            .map_err(|err| eyre!("Failed to query current time: {:?}", err))?;

        let inner_message = encode_with_selector(
            "solverUpdate(uint256,bytes,uint256)",
            timestamp,
            weights,
            price,
        );

        let tail_call_data = Bytes::default();

        let message = call_connector_message(
            timestamp,
            self.custody_id,
            OTC_INDEX_CONNECTOR_NAME,
            self.index_address,
            &inner_message,
            &tail_call_data,
        );

        let proof = vec![]; // index is already in whitelisted so empty proof OK

        let verification_data = self.get_verification_data(
            OTC_INDEX_SOLVER_WEIGHTS_SET_STATE,
            timestamp,
            &proof,
            &message,
        )?;

        let otc = OTCCustody::new(self.custody_address, provider);
        let receipt = otc
            .callConnector(
                String::from(OTC_INDEX_CONNECTOR_NAME),
                self.index_address,
                message,
                tail_call_data,
                verification_data,
            )
            .from(from_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        if !receipt.status() {
            Err(eyre!("Failed to set curator weights: {:?}", receipt))?;
        }

        Ok(receipt)
    }

    pub async fn mint_index_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        mint_to: Address,
        amount: U256,
        sequence_number: U256,
    ) -> eyre::Result<TransactionReceipt> {
        let timestamp = get_last_block_timestamp(&provider)
            .await
            .map_err(|err| eyre!("Failed to query current time: {:?}", err))?;

        let inner_message = encode_mint_call(mint_to, amount, sequence_number);
        let tail_call_data = Bytes::default();
        let nonce: u64 = pending_nonce(&provider, from_address).await?;

        let message = call_connector_message(
            timestamp,
            self.custody_id,
            OTC_INDEX_CONNECTOR_NAME,
            self.index_address,
            &inner_message,
            &tail_call_data,
        );

        let proof = vec![]; // index is already in whitelisted so empty proof OK

        let verification_data =
            self.get_verification_data(OTC_INDEX_MINT_CURATOR_STATE, timestamp, &proof, &message)?;

        let otc = OTCCustody::new(self.custody_address, provider);
        let receipt = otc
            .callConnector(
                String::from(OTC_INDEX_CONNECTOR_NAME),
                self.index_address,
                message,
                tail_call_data,
                verification_data,
            )
            .from(from_address)
            .nonce(nonce)
            .send()
            .await?
            .get_receipt()
            .await?;

        if !receipt.status() {
            Err(eyre!("Failed to mint index: {:?}", receipt))?;
        }

        Ok(receipt)
    }

    async fn route_collateral_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        route_to: Address,
        amount: U256,
        custody_state: u8,
        proof: &Vec<B256>,
    ) -> eyre::Result<TransactionReceipt> {
        let timestamp = get_last_block_timestamp(&provider)
            .await
            .map_err(|err| eyre!("Failed to query current time: {:?}", err))?;

        let message = custody_to_address_message(
            timestamp,
            self.custody_id,
            self.index_deploy_data.collateral_token,
            route_to,
            amount,
        );

        let verification_data =
            self.get_verification_data(custody_state, timestamp, proof, &message)?;

        let otc = OTCCustody::new(self.custody_address, provider);
        let receipt = otc
            .custodyToAddress(
                self.index_deploy_data.collateral_token,
                route_to,
                amount,
                verification_data,
            )
            .from(from_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        if !receipt.status() {
            Err(eyre!("Failed to deploy index: {:?}", receipt))?;
        }

        Ok(receipt)
    }

    pub async fn route_collateral_for_trading_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        amount: U256,
    ) -> eyre::Result<TransactionReceipt> {
        self.route_collateral_from(
            provider,
            from_address,
            self.trade_route,
            amount,
            OTC_TRADE_ROUTE_CUSTODY_STATE,
            &self.trade_route_proof,
        )
        .await
        .map_err(|err| eyre!("Failed to route collateral for trading: {:?}", err))
    }

    pub async fn withdraw_collateral_from(
        &self,
        provider: impl Provider,
        from_address: Address,
        amount: U256,
    ) -> eyre::Result<TransactionReceipt> {
        self.route_collateral_from(
            provider,
            from_address,
            self.withdraw_route,
            amount,
            OTC_WITHDRAW_ROUTE_CUSTODY_STATE,
            &self.withdraw_route_proof,
        )
        .await
        .map_err(|err| eyre!("Failed to withdraw collateral: {:?}", err))
    }
}

#[async_trait]
impl CustodyClientMethods for IndexInstance {
    fn get_custody_id(&self) -> B256 {
        self.custody_id
    }

    fn get_custody_address(&self) -> &Address {
        &self.custody_address
    }

    fn get_collateral_token_address(&self) -> &Address {
        &self.index_deploy_data.collateral_token
    }

    fn get_collateral_token_precision(&self) -> u8 {
        self.index_deploy_data.collateral_token_precision.to::<u8>()
    }

    async fn route_collateral_to_from(
        &self,
        provider: DynProvider,
        from_address: &Address,
        to_address: &Address,
        token_address: &Address,
        amount: U256,
    ) -> eyre::Result<TransactionReceipt> {
        if !self.index_deploy_data.collateral_token.eq(token_address) {
            Err(eyre!(
                "Failed to route collateral: Invalid collateral token {}",
                token_address
            ))?
        }

        if self.trade_route.eq(to_address) {
            self.route_collateral_for_trading_from(provider, *from_address, amount)
                .await
        } else if self.withdraw_route.eq(to_address) {
            self.withdraw_collateral_from(provider, *from_address, amount)
                .await
        } else {
            Err(eyre!(
                "Failed to route collateral: Invalid address {}",
                to_address
            ))
        }
    }
}
