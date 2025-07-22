use crate::designation_details::EvmDesignationDetails;
use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::bits::{Address, Amount, Symbol};

const BRIDGE_TYPE: &str = "EVM";

pub struct EvmCollateralDesignation {
    pub name: Symbol,              //< e.g. "ARBITRUM", or "BASE"
    pub collateral_symbol: Symbol, //< should be "USDC" (in future could also be "USDT")
    pub full_name: Symbol,         //< e.g. "EVM:ARBITRUM:USDC"
    pub chain_id: u64,             //< chain ID for this designation
    pub token_address: Address,    //< token contract address for this designation
}

impl EvmCollateralDesignation {
    pub fn new(
        name: Symbol,
        collateral_symbol: Symbol,
        chain_id: u64,
        token_address: Address,
    ) -> Self {
        let full_name = format!("EVM:{}:{}", name, collateral_symbol).into();
        Self {
            name,
            collateral_symbol,
            full_name,
            chain_id,
            token_address,
        }
    }

    pub fn arbitrum_usdc(token_address: Address) -> Self {
        Self::new("ARBITRUM".into(), "USDC".into(), 42161, token_address)
    }

    pub fn base_usdc(token_address: Address) -> Self {
        Self::new("BASE".into(), "USDC".into(), 8453, token_address)
    }
}

impl CollateralDesignation for EvmCollateralDesignation {
    fn get_type(&self) -> Symbol {
        BRIDGE_TYPE.into()
    }
    fn get_name(&self) -> Symbol {
        self.name.clone()
    }
    fn get_collateral_symbol(&self) -> Symbol {
        self.collateral_symbol.clone()
    }
    fn get_full_name(&self) -> Symbol {
        self.full_name.clone() //< for preformance better to pre-construct than format on-demand
    }
    fn get_balance(&self) -> Amount {
        // TODO: Should enqueue ChainCommand to retrieve balance at given designation
        // i.e. balance of our custody on network X in currency Y
        todo!("Tell the balance of collateral symbol (e.g. USDC) in that designation")
    }
}

impl EvmDesignationDetails for EvmCollateralDesignation {
    fn get_chain_id(&self) -> u64 {
        self.chain_id
    }

    fn get_token_address(&self) -> Address {
        self.token_address
    }

    fn is_cross_chain(&self, other: &Self) -> bool {
        self.chain_id != other.chain_id
    }
}
