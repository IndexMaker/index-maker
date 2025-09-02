use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    collateral::collateral_position::CollateralPosition, solver::solver_order::SolverOrderAssetLot,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixHeader {
    pub msg_type: String,       // Type of message, e.g. "NewOrderSingle"
    pub sender_comp_id: String, // Public key of sender
    pub target_comp_id: String, // Public key of receiver
    pub seq_num: u32,           // Message sequence number
    //    ResetSeqNum: u8,       // Boolean that determines if SeqNum must be ignored (this will invalidate Disputes)
    pub timestamp: DateTime<Utc>,
    //    CustodyId: String,
}

impl FixHeader {
    pub fn new(msg_type: String) -> Self {
        Self {
            msg_type: msg_type,
            sender_comp_id: "".to_string(),
            target_comp_id: "".to_string(),
            timestamp: Utc::now(),
            seq_num: 0,
        }
    }

    pub fn add_sender(&mut self, sender: String) {
        self.sender_comp_id = sender;
    }

    pub fn add_target(&mut self, target: String) {
        self.target_comp_id = target;
    }

    pub fn add_seq_num(&mut self, seq_num: u32) {
        self.seq_num = seq_num;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixTrailer {
    pub public_key: Vec<String>, // Public key of sender
    pub signature: Vec<String>,  // Public key of receiver
                                 //    RateLimitInterval: u32,           // Message sequence number
                                 //    RateLimitCount: u32,
}

impl FixTrailer {
    pub fn new() -> Self {
        Self {
            public_key: Vec::new(),
            signature: Vec::new(),
        }
    }

    pub fn add_public(&mut self, public_key: String) {
        self.public_key.push(public_key);
    }

    pub fn add_signature(&mut self, signature: String) {
        self.signature.push(signature);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum RequestBody {
    // NewOrderBody {
    //     client_order_id: String,
    //     symbol: String,
    //     side: String,
    //     price: String,
    //     order_quantity: String,
    //     order_type: String,
    // },
    NewIndexOrderBody {
        client_order_id: String,
        symbol: String,
        side: String,
        amount: String,
    },
    CancelIndexOrderBody {
        client_order_id: String,
        symbol: String,
        amount: String,
    },
    NewQuoteRequestBody {
        client_quote_id: String,
        symbol: String,
        side: String,
        amount: String,
    },
    CancelQuoteRequestBody {
        client_quote_id: String,
        symbol: String,
    },
    AccountToCustodyBody,
    CustodyToAccountBody,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ResponseBody {
    ACKBody {
        ref_seq_num: u32,
    },
    NAKBody {
        ref_seq_num: u32,
        reason: String,
    },
    NewOrderBody {
        status: String,
        client_order_id: String,
    },
    NewOrderFailBody {
        status: String,
        client_order_id: String,
        reason: String,
    },
    IndexQuoteRequestBody {
        status: String,
        client_quote_id: String,
    },
    IndexQuoteRequestFailBody {
        status: String,
        client_quote_id: String,
        reason: String,
    },
    IndexOrderFillBody {
        client_order_id: String,
        filled_quantity: String,
        collateral_spent: String,
        collateral_remaining: String,
        fill_rate: String,
        status: String,
    },
    IndexQuoteBody {
        client_quote_id: String,
        quantity_possible: String,
    },
    MintInvoiceBody {
        client_order_id: String,
        payment_id: String,
        symbol: String,
        filled_quantity: String,
        total_amount: String,
        amount_paid: String,
        amount_remaining: String,
        management_fee: String,
        assets_value: String,
        exchange_fee: String,
        fill_rate: String,
        lots: Vec<SolverOrderAssetLot>,
        position: CollateralPosition,
        timestamp: DateTime<Utc>,
    },
    AccountToCustodyBody,
    CustodyToAccountBody,
    // ExecReportBody {
    //     client_order_id: String,
    //     execution_type: String,
    //     order_status: String,
    //     side: String,
    //     order_quantity: String,
    //     price: String,
    //     cummulative_quantity: String,
    // },
}
