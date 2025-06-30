pub mod app {
    pub mod basket_manager;
    pub mod batch_manager;
    pub mod collateral_manager;
    pub mod config;
    pub mod index_order_manager;
    pub mod market_data;
    pub mod order_sender;
    pub mod quote_request_manager;
    pub mod simple_chain;
    pub mod simple_router;
    pub mod simple_server;
    pub mod axum_server;
    pub mod simple_solver;
    pub mod solver;
    pub mod timestamp_ids;
}

pub mod blockchain {
    pub mod chain_connector;
}

pub mod collateral {
    pub mod collateral_manager;
    pub mod collateral_position;
    pub mod collateral_router;
}

pub mod index {
    pub mod basket;
    pub mod basket_manager;
}

pub mod server {
    pub mod server;
    pub mod fix_messages;
    pub mod requests;
    pub mod responses;
    pub mod example_plugin;
}

pub mod solver {
    pub mod batch_manager;
    pub mod index_order;
    pub mod index_order_manager;
    pub mod index_quote;
    pub mod index_quote_manager;
    pub mod mint_invoice;
    pub mod solver;
    pub mod solver_order;
    pub mod solver_quote;
    pub mod solvers {
        pub mod simple_solver;
    }
}
