use crate::designation_details::EvmDesignationDetails;
use crate::across_deposit::{USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS};
use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::bits::{Address, Amount, Symbol};

const BRIDGE_TYPE: &str = "EVM";

pub struct EvmCollateralDesignation {
    pub name: Symbol,              //< e.g. "ARBITRUM", or "BASE"
    pub collateral_symbol: Symbol, //< should be "USDC" (in future could also be "USDT")
    pub full_name: Symbol,         //< e.g. "EVM:ARBITRUM:USDC"
    pub chain_id: u64,             //< chain ID for this designation
    pub collateral_address: Address,    //< address of USDC contract
    pub address: Address,          //< address of the designation (wallet or contract)
}

impl EvmCollateralDesignation {
    pub fn new(
        name: Symbol,
        collateral_symbol: Symbol,
        chain_id: u64,
        collateral_address: Address,
        address: Address,
    ) -> Self {
        let full_name = format!("EVM:{}:{}", name, collateral_symbol).into();
        Self {
            name,
            collateral_symbol,
            full_name,
            chain_id,
            collateral_address,
            address,
        }
    }

    pub fn arbitrum_usdc(address: Address) -> Self {
        Self::new("ARBITRUM".into(), "USDC".into(), 42161, USDC_ARBITRUM_ADDRESS, address)
    }

    pub fn base_usdc(address: Address) -> Self {
        Self::new("BASE".into(), "USDC".into(), 8453, USDC_BASE_ADDRESS, address)
    }

    pub fn get_wallet_address(&self) -> Address {
        self.address
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
        self.collateral_address
    }

    fn is_cross_chain(&self, other: &Self) -> bool {
        self.chain_id != other.chain_id
    }
}
