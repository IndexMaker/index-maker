use std::sync::Arc;

use alloy::{
    primitives::{Address, Bytes, B256, U256},
    providers::Provider,
};
use eyre::{eyre, OptionExt};

use crate::{
    connector_util::deploy_connector_message,
    contracts::{IndexFactory, OTCCustody, VerificationData},
    custody_authority::CustodyAuthority,
    custody_helper::{CAHelper, CAItem, Party},
    index::{
        index::IndexInstance,
        index_helper::{
            IndexDeployData, OTC_INDEX_CONNECTOR_TYPE, OTC_INDEX_DEPLOY_CUSTODY_STATE,
            OTC_TRADE_ROUTE_CUSTODY_STATE, OTC_WITHDRAW_ROUTE_CUSTODY_STATE,
        },
    },
    util::get_last_block_timestamp,
};

pub struct IndexDeployment {
    index_deploy_data: IndexDeployData,
    index_operator: CustodyAuthority,
    index_factory_address: Address,
    custody_address: Address,
    trade_route: Address,
    withdraw_route: Address,
    custody_id: B256,
    ca_items: Vec<CAItem>,
    index_deploy_proof: Vec<B256>,
    trade_route_proof: Vec<B256>,
    withdraw_route_proof: Vec<B256>,
}

impl IndexDeployment {
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

    pub fn builder_for(
        index_operator: CustodyAuthority,
        chain_id: u32,
        index_factory_address: Address,
        custody_address: Address,
        trade_route: Address,
        withdraw_route: Address,
    ) -> Arc<IndexDeploymentBuilder> {
        Arc::new(IndexDeploymentBuilder {
            index_operator,
            chain_id,
            index_factory_address,
            custody_address,
            trade_route,
            withdraw_route,
        })
    }

    pub fn get_ca_items(&self) -> &Vec<CAItem> {
        &self.ca_items
    }

    pub fn get_deploy_data(&self) -> &IndexDeployData {
        &self.index_deploy_data
    }

    /// Obtain existing index instance
    pub fn into_index_at(self, index_address: Address) -> IndexInstance {
        IndexInstance {
            index_deploy_data: self.index_deploy_data,
            index_operator: self.index_operator,
            custody_address: self.custody_address,
            index_address,
            trade_route: self.trade_route,
            withdraw_route: self.withdraw_route,
            custody_id: self.custody_id,
            trade_route_proof: self.trade_route_proof,
            withdraw_route_proof: self.withdraw_route_proof,
        }
    }

    /// Deploy new index instance
    pub async fn deploy_from(
        self,
        provider: &impl Provider,
        from_address: Address,
    ) -> eyre::Result<IndexInstance> {
        let timestamp = get_last_block_timestamp(provider)
            .await
            .map_err(|err| eyre!("Failed to query current time: {:?}", err))?;

        let index_deploy_calldata = self.index_deploy_data.clone().to_encoded_params();

        let message = deploy_connector_message(
            timestamp,
            self.custody_id,
            OTC_INDEX_CONNECTOR_TYPE,
            self.index_factory_address,
            &index_deploy_calldata,
        );

        let verification_data = self.get_verification_data(
            OTC_INDEX_DEPLOY_CUSTODY_STATE,
            timestamp,
            &self.index_deploy_proof,
            &message,
        )?;

        let otc = OTCCustody::new(self.custody_address, provider);
        let receipt = otc
            .deployConnector(
                String::from(OTC_INDEX_CONNECTOR_TYPE),
                self.index_factory_address,
                index_deploy_calldata,
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

        let log_entry = receipt
            .logs()
            .iter()
            .find(|&x| x.address() == self.index_factory_address)
            .ok_or_eyre("Failed to find index deployment event")?;

        let event = log_entry
            .log_decode::<IndexFactory::IndexDeployed>()
            .map_err(|err| eyre!("Failed to parse log entry: {:?}", err))?;

        let index_address = event.inner.indexAddress;

        let is_whitelisted = otc
            .isConnectorWhitelisted(index_address)
            .call()
            .await
            .map_err(|err| eyre!("Failed to query index: {:?}", err))?;

        if !is_whitelisted {
            tracing::warn!("Index is not whitelisted: {}", index_address);
        }

        Ok(IndexInstance {
            index_deploy_data: self.index_deploy_data,
            index_operator: self.index_operator,
            custody_address: self.custody_address,
            index_address,
            trade_route: self.trade_route,
            withdraw_route: self.withdraw_route,
            custody_id: self.custody_id,
            trade_route_proof: self.trade_route_proof,
            withdraw_route_proof: self.withdraw_route_proof,
        })
    }
}

pub struct IndexDeploymentBuilder {
    index_operator: CustodyAuthority,
    chain_id: u32,
    index_factory_address: Address,
    custody_address: Address,
    trade_route: Address,
    withdraw_route: Address,
}

impl IndexDeploymentBuilder {
    fn get_party(&self) -> eyre::Result<Party> {
        self.index_operator.get_party()
    }

    pub fn build(&self, index_deploy_data: IndexDeployData) -> eyre::Result<IndexDeployment> {
        let mut ca = CAHelper::new(self.chain_id, self.custody_address);

        let party = self.get_party()?;
        let index_deploy_calldata = index_deploy_data.clone().to_encoded_params();

        // Allow ourselves to deploy index
        let index_deploy_leaf_index = ca.deploy_connector(
            OTC_INDEX_CONNECTOR_TYPE,
            self.index_factory_address,
            &index_deploy_calldata,
            OTC_INDEX_DEPLOY_CUSTODY_STATE,
            party.clone(),
        );

        // Allow ourselves to route collateral towards trading venue
        let trade_route_leaf_index = ca.custody_to_address(
            self.trade_route,
            OTC_TRADE_ROUTE_CUSTODY_STATE,
            party.clone(),
        );

        // Allow ourselves to withdraw
        // ===
        // TODO: it's actually OTCIndex that will withdraw, so what should we
        // put on ACL here? We need to enable OTCIndex to withdraw.
        // let withdraw_route_leaf_index = ca.custody_to_address(
        //     self.withdraw_route,
        //     OTC_WITHDRAW_ROUTE_CUSTODY_STATE,
        //     party.clone(),
        // );

        let custody_id = B256::from_slice(&ca.get_custody_id());
        let ca_items = ca.get_ca_items();

        let index_deploy_proof: Vec<B256> = ca
            .get_merkle_proof(index_deploy_leaf_index)
            .into_iter()
            .map(|arr| B256::from_slice(&arr))
            .collect();

        let trade_route_proof: Vec<B256> = ca
            .get_merkle_proof(trade_route_leaf_index)
            .into_iter()
            .map(|arr| B256::from_slice(&arr))
            .collect();

        // FIXME: MerkleTree fails to build correctly!
        let withdraw_route_proof = vec![];
        // let withdraw_route_proof: Vec<B256> = ca
        //     .get_merkle_proof(withdraw_route_leaf_index)
        //     .into_iter()
        //     .map(|arr| B256::from_slice(&arr))
        //     .collect();

        Ok(IndexDeployment {
            index_deploy_data,
            index_operator: self.index_operator.clone(),
            index_factory_address: self.index_factory_address,
            custody_address: self.custody_address,
            trade_route: self.trade_route,
            withdraw_route: self.withdraw_route,
            custody_id,
            ca_items,
            index_deploy_proof,
            trade_route_proof,
            withdraw_route_proof,
        })
    }
}

#[cfg(test)]
pub mod test {
    use alloy::{
        primitives::{address, Bytes, U256},
        providers::ProviderBuilder,
    };

    use crate::{
        custody_authority::CustodyAuthority,
        index::{index_deployment::IndexDeployment, index_helper::IndexDeployData},
    };

    /// An example of how we can deploy new index, and then use it.
    ///
    /// Example demonstrates the process of configuring brand new index, and
    /// then deploying it, and eventually using its smart-contract methods.
    ///
    #[tokio::test]
    #[ignore = "This is only conceptual example that cannot be run, but should compile"]
    async fn test_index_deployment_sbe() {
        let index_operator = CustodyAuthority::new(|| String::from("abc"));

        let builder = IndexDeployment::builder_for(
            index_operator,
            1,
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
        );

        let index_deploy_data = IndexDeployData {
            name: String::from("Top 100"),
            symbol: String::from("SY100"),
            collateral_token: address!("0x1111111111111111111122222222222222222222"),
            collateral_token_precision: U256::from(6),
            management_fee: U256::from(1_000_000),
            performance_fee: U256::from(1_000_000),
            max_mint_per_block: U256::from(1_000_000),
            max_redeem_per_block: U256::from(1_000_000),
            vote_threshold: U256::from(1_000_000),
            vote_period: U256::from(1_000_000),
            initial_price: U256::from(1_000_000),
        };

        let index_deployment = builder
            .build(index_deploy_data)
            .expect("Failed to build deployment");

        let provider = ProviderBuilder::new()
            .connect("nowhere")
            .await
            .expect("Failed to connect to RPC");

        let from_address = address!("0x1111111111111111111122222222222222222222");

        let index = index_deployment
            .deploy_from(&provider, from_address)
            .await
            .expect("Failed to deploy index");

        index
            .set_currator_weights_from(&provider, from_address, &Bytes::default(), U256::ZERO)
            .await
            .expect("Failed to set curator weights");

        index
            .solver_weights_set_from(&provider, from_address, &Bytes::default(), U256::ZERO)
            .await
            .expect("Failed to set solver weights");

        index
            .route_collateral_for_trading_from(&provider, from_address, U256::ZERO)
            .await
            .expect("Failed to route collateral");

        index
            .mint_index_from(
                &provider,
                from_address,
                address!("0x1111111111111111111122222222222222222222"),
                U256::ZERO,
                U256::ZERO,
            )
            .await
            .expect("Failed to mint to address");
    }

    /// An example of how we can use index that was previously deployed.
    ///
    /// Here we require that we re-build ACL, so that we get Custody ID and
    /// individual Markle-Proofs. We must provide same parameters as when
    /// index was originally deployed.
    ///
    #[tokio::test]
    #[ignore = "This is only conceptual example that cannot be run, but should compile"]
    async fn test_into_index_sbe() {
        let index_operator = CustodyAuthority::new(|| String::from("abc"));

        let builder = IndexDeployment::builder_for(
            index_operator,
            1,
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
            address!("0x1111111111111111111122222222222222222222"),
        );

        let index_deploy_data = IndexDeployData {
            name: String::from("Top 100"),
            symbol: String::from("SY100"),
            collateral_token: address!("0x1111111111111111111122222222222222222222"),
            collateral_token_precision: U256::from(6),
            management_fee: U256::from(1_000_000),
            performance_fee: U256::from(1_000_000),
            max_mint_per_block: U256::from(1_000_000),
            max_redeem_per_block: U256::from(1_000_000),
            vote_threshold: U256::from(1_000_000),
            vote_period: U256::from(1_000_000),
            initial_price: U256::from(1_000_000),
        };

        let index_deployment = builder
            .build(index_deploy_data)
            .expect("Failed to build deployment");

        let index =
            index_deployment.into_index_at(address!("0x1111111111111111111122222222222222222222"));

        let provider = ProviderBuilder::new()
            .connect("nowhere")
            .await
            .expect("Failed to connect to RPC");

        let from_address = address!("0x1111111111111111111122222222222222222222");

        index
            .withdraw_collateral_from(&provider, from_address, U256::ZERO)
            .await
            .expect("Failed to withdraw collateral");
    }
}
