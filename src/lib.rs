pub mod assets {
    pub mod asset;
}

pub mod blockchain {
    pub mod chain_connector;
}

pub mod core {
    pub mod bits;
    pub mod decimal_ext;
    pub mod decimal_macros;
    pub mod deque_macros;
    pub mod functional;
    pub mod hashmap_macros;

    #[cfg(test)]
    pub mod test_util;
}

pub mod index {
    pub mod basket;
    pub mod basket_manager;
}

pub mod market_data {
    pub mod market_data_connector;
    pub mod price_tracker;

    pub mod order_book {
        pub mod order_book;
        pub mod order_book_manager;
    }
}

pub mod order_sender {
    pub mod order_connector;
    pub mod order_tracker;
}

pub mod server {
    pub mod server;
}

pub mod solver {
    pub mod batch_manager;
    pub mod index_order;
    pub mod index_order_manager;
    pub mod index_quote;
    pub mod index_quote_manager;
    pub mod inventory_manager;
    pub mod position;
    pub mod solver;
    pub mod solvers {
        pub mod simple_solver;
    }
}
