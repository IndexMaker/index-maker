pub mod app {
    pub mod application;
    pub mod basket_manager;
    pub mod batch_manager;
    pub mod chain_connector;
    pub mod collateral_manager;
    pub mod collateral_router;
    pub mod config;
    pub mod config_loader;
    pub mod fix_server;
    pub mod index_order_manager;
    pub mod market_data;
    pub mod mint_invoice_manager;
    pub mod order_sender;
    pub mod secret_provider;

    pub mod query_service;
    pub mod quote_request_manager;
    pub mod simple_chain;
    pub mod simple_router;
    pub mod simple_sender;
    pub mod simple_server;
    pub mod simple_solver;
    pub mod solver;
    pub mod timestamp_ids;
}

pub mod collateral {
    pub mod collateral_manager;
    pub mod collateral_position;
}

pub mod query {
    pub mod query_service;
    pub mod query_service_state;
}

pub mod server {
    pub mod server;
    pub mod fix {
        pub mod messages;
        pub mod rate_limit_config;
        pub mod requests;
        pub mod responses;
        pub mod server;
        pub mod server_plugin;
    }
}

pub mod solver {
    pub mod batch_manager;
    pub mod index_order;
    pub mod index_order_manager;
    pub mod index_quote;
    pub mod index_quote_manager;
    pub mod mint_invoice;
    pub mod mint_invoice_manager;
    pub mod solver;
    pub mod solver_order {
        pub mod solver_order;
        pub mod solver_order_serde;
    }
    pub mod solver_quote;
    pub mod solvers {
        pub mod simple_solver;
    }
}

pub mod cli;
