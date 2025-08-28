use std::sync::Arc;

use alloy::{
    consensus::BlockHeader,
    network::BlockResponse,
    primitives::{Address, Bytes, B256, U256},
    providers::Provider,
    rpc::types::BlockNumberOrTag,
};
use k256::elliptic_curve::zeroize::Zeroize;

use crate::{
    contracts::{OTCCustody, SchnorrCAKey, SchnorrSignature, VerificationData},
    custody_helper::{CAHelper, Party},
    index_helper::{deploy_connector_message, IndexDeployData},
    schnorr::{pubkey_parity_27_28, schnorr_sign_per_contract, signing_key_bytes_from_hex},
};

pub trait IndexOperatorMethods {
    fn get_party(&self) -> eyre::Result<Party>;
    fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData>;
}

pub struct IndexOperatorImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    get_signing_key: GetSigningKeyFn,
}

impl<GetSigningKeyFn> IndexOperatorImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    pub fn new(get_signing_key: GetSigningKeyFn) -> Self {
        Self { get_signing_key }
    }
}

impl<GetSigningKeyFn> IndexOperatorMethods for IndexOperatorImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    fn get_party(&self) -> eyre::Result<Party> {
        let signing_key_hex = (self.get_signing_key)();
        let mut signing_key_bytes = signing_key_bytes_from_hex(signing_key_hex)?;
        let (parity, x) = pubkey_parity_27_28(&signing_key_bytes)?;
        signing_key_bytes.zeroize();
        Ok(Party { parity, x })
    }

    fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData> {
        let signing_key_hex = (self.get_signing_key)();
        let mut signing_key_bytes = signing_key_bytes_from_hex(signing_key_hex)?;
        let (parity, x) = pubkey_parity_27_28(&signing_key_bytes)?;
        let (e, s) = schnorr_sign_per_contract(&signing_key_bytes, parity, x, message)?;
        signing_key_bytes.zeroize();
        Ok(VerificationData {
            id: custody_id,
            state: custody_state,
            timestamp,
            pubKey: SchnorrCAKey { parity, x },
            sig: SchnorrSignature { e, s },
            merkleProof: proof.clone(),
        })
    }
}

#[derive(Clone)]
pub struct IndexOperator(Arc<dyn IndexOperatorMethods + Send + Sync + 'static>);

impl IndexOperator {
    pub fn new_from_fn(
        get_signing_key: impl Fn() -> String + Send + Sync + 'static,
    ) -> IndexOperator {
        IndexOperator(Arc::new(IndexOperatorImpl::new(get_signing_key)))
    }

    pub fn get_party(&self) -> eyre::Result<Party> {
        self.0.get_party()
    }

    pub fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData> {
        self.0
            .get_verification_data(custody_id, custody_state, timestamp, proof, message)
    }
}

const OTC_INDEX_CONNECTOR_TYPE: &str = "OTCIndex";
const OTC_INDEX_DEPLOY_CUSTODY_STATE: u8 = 0;
const OTC_TRADE_ROUTE_CUSTODY_STATE: u8 = 0;
const OTC_WITHDRAW_ROUTE_CUSTODY_STATE: u8 = 0;

pub struct IndexDeployment {
    index_operator: IndexOperator,
    chain_id: u32,
    index_factory_address: Address,
    otc_custody_address: Address,
    index_deploy_calldata: Bytes,
    custody_id: B256,
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
        index_operator: IndexOperator,
        chain_id: u32,
        index_factory_address: Address,
        otc_custody_address: Address,
        trade_route: Address,
        withdraw_route: Address,
    ) -> Arc<IndexDeploymentBuilder> {
        Arc::new(IndexDeploymentBuilder {
            index_operator,
            chain_id,
            index_factory_address,
            otc_custody_address,
            trade_route,
            withdraw_route,
        })
    }

    pub async fn deploy_from(
        &self,
        provider: impl Provider,
        from_address: Address,
    ) -> eyre::Result<()> {
        let timestamp: U256 = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .map(|b| U256::from(b.header().timestamp()))
            .unwrap_or(U256::ZERO);

        let message = deploy_connector_message(
            timestamp,
            self.custody_id,
            OTC_INDEX_CONNECTOR_TYPE,
            self.index_factory_address,
            &self.index_deploy_calldata,
        );

        let verification_data = self.get_verification_data(
            OTC_INDEX_DEPLOY_CUSTODY_STATE,
            timestamp,
            &self.index_deploy_proof,
            &message,
        )?;

        let otc = OTCCustody::new(self.otc_custody_address, provider);
        let rc = otc
            .deployConnector(
                String::from(OTC_INDEX_CONNECTOR_TYPE),
                self.index_factory_address,
                self.index_deploy_calldata.clone(),
                verification_data,
            )
            .from(from_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(())
    }
}

pub struct IndexDeploymentBuilder {
    index_operator: IndexOperator,
    chain_id: u32,
    index_factory_address: Address,
    otc_custody_address: Address,
    trade_route: Address,
    withdraw_route: Address,
}

impl IndexDeploymentBuilder {
    fn get_party(&self) -> eyre::Result<Party> {
        self.index_operator.get_party()
    }

    pub fn build(&self, index_deploy_data: IndexDeployData) -> eyre::Result<IndexDeployment> {
        let mut ca = CAHelper::new(self.chain_id, self.otc_custody_address);

        let party = self.get_party()?;
        let index_deploy_calldata = index_deploy_data.to_encoded_params();

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
        let withdraw_route_leaf_index = ca.custody_to_address(
            self.withdraw_route,
            OTC_WITHDRAW_ROUTE_CUSTODY_STATE,
            party.clone(),
        );

        let custody_id = B256::from_slice(&ca.get_custody_id());

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

        let withdraw_route_proof: Vec<B256> = ca
            .get_merkle_proof(withdraw_route_leaf_index)
            .into_iter()
            .map(|arr| B256::from_slice(&arr))
            .collect();

        Ok(IndexDeployment {
            index_operator: self.index_operator.clone(),
            chain_id: self.chain_id,
            index_factory_address: self.index_factory_address,
            otc_custody_address: self.otc_custody_address,
            index_deploy_calldata,
            custody_id,
            index_deploy_proof,
            trade_route_proof,
            withdraw_route_proof,
        })
    }
}

#[cfg(test)]
pub mod test {
    use alloy::{
        primitives::{address, U256},
        providers::ProviderBuilder,
    };

    use crate::index::{IndexDeployData, IndexDeployment, IndexOperator};

    #[tokio::test]
    #[ignore]
    async fn test_index_deployment_sbe() {
        let index_operator = IndexOperator::new_from_fn(|| String::from("abc"));

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
            .connect("")
            .await
            .expect("Failed to connect to RPC");

        index_deployment
            .deploy_from(
                provider,
                address!("0x1111111111111111111122222222222222222222"),
            )
            .await
            .expect("Failed to deploy index");
    }
}
