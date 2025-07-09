use chrono::{DateTime, Utc};

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::{
        bits::{Address, Amount, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    index::basket::{Basket, BasketDefinition},
};

pub struct EvmConnector {
    observer: SingleObserver<ChainNotification>,
    //... add more data as needed
}

impl EvmConnector {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
        }
    }

    pub fn connect(&mut self) {
        todo!("Connect to blockchain, receive events");
    }

    fn notify_curator_weights_set(&self, symbol: Symbol, basket_definition: BasketDefinition) {
        self.observer
            .publish_single(ChainNotification::CuratorWeightsSet(
                symbol,
                basket_definition,
            ));
    }

    fn notify_deposit(
        &self,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) {
        self.observer.publish_single(ChainNotification::Deposit {
            chain_id,
            address,
            amount,
            timestamp,
        });
    }

    fn notify_withdrawal_request(
        &self,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) {
        self.observer
            .publish_single(ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            });
    }
}

impl ChainConnector for EvmConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: std::sync::Arc<Basket>) {
        todo!("Call corresponding smart-contract method");
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        todo!("Call corresponding smart-contract method");
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, receipient: Address) {
        todo!("Call corresponding smart-contract method");
    }

    fn withdraw(
        &self,
        chain_id: u32,
        receipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        todo!("Call corresponding smart-contract method");
    }
}

impl IntoObservableSingle<ChainNotification> for EvmConnector {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<ChainNotification> {
        &mut self.observer
    }
}
