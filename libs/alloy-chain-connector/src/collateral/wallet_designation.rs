use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::bits::{Address, Amount, Symbol};

pub struct WalletCollateralDesignation {
    designation_type: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
    full_name: Symbol,
    chain_id: u32,
    address: Address,
    token_address: Address,
}

impl WalletCollateralDesignation {
    pub fn new(
        designation_type: Symbol,
        name: Symbol,
        collateral_symbol: Symbol,
        chain_id: u32,
        address: Address,
        token_address: Address,
    ) -> Self {
        let full_name = Symbol::from(format!(
            "{}:{}:{}",
            designation_type, name, collateral_symbol
        ));
        Self {
            designation_type,
            name,
            collateral_symbol,
            full_name,
            chain_id,
            address,
            token_address,
        }
    }

    pub fn get_chain_id(&self) -> u32 {
        self.chain_id
    }

    pub fn get_address(&self) -> Address {
        self.address
    }

    pub fn get_token_address(&self) -> Address {
        self.token_address
    }
}

impl CollateralDesignation for WalletCollateralDesignation {
    fn get_type(&self) -> Symbol {
        self.designation_type.clone()
    }

    fn get_name(&self) -> Symbol {
        self.name.clone()
    }

    fn get_collateral_symbol(&self) -> Symbol {
        self.collateral_symbol.clone()
    }

    fn get_full_name(&self) -> Symbol {
        self.full_name.clone()
    }

    fn get_balance(&self) -> Amount {
        todo!()
    }
}
