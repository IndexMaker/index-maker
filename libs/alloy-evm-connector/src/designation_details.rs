/// Trait for EVM-specific designation functionality
pub trait EvmDesignationDetails {
    fn get_chain_id(&self) -> u64;
    fn get_token_address(&self) -> symm_core::core::bits::Address;
    fn is_cross_chain(&self, other: &Self) -> bool;
}
