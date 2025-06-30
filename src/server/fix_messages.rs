use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixHeader {
    pub MsgType: String,       // Type of message, e.g. "NewOrderSingle"
    pub SenderCompID: String,  // Public key of sender
    pub TargetCompID: String,  // Public key of receiver
    pub SeqNum: u32,           // Message sequence number
//    ResetSeqNum: u8,       // Boolean that determines if SeqNum must be ignored (this will invalidate Disputes)
//    SendingTime: u64,      // Unix timestamp of sending time
//    CustodyId: String,
}

impl FixHeader {
    pub fn new(MsgType: String) -> Self{
        Self {
            MsgType,
            SenderCompID: "".to_string(),
            TargetCompID: "".to_string(),
            SeqNum: 0,
        }
    }

    pub fn add_sender(&mut self, sender: String) {
        self.SenderCompID = sender;
    }

    pub fn add_target(&mut self, target: String) {
        self.TargetCompID = target;
    }

    pub fn add_seq_num(&mut self, SeqNum: u32) {
        self.SeqNum = SeqNum;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixTrailer {
    pub PublicKey: Vec<String>,  // Public key of sender
    pub Signature: Vec<String>,  // Public key of receiver
//    RateLimitInterval: u32,           // Message sequence number
//    RateLimitCount: u32,
}

impl FixTrailer {
    pub fn new() -> Self{
        Self {
            PublicKey: Vec::new(),
            Signature: Vec::new(),
        }
    }

    pub fn add_public(&mut self, public_key: String) {
        self.PublicKey.push(public_key);
    }

    pub fn add_signature(&mut self, signature: String) {
        self.Signature.push(signature);
    }
}

#[derive(Serialize, Deserialize, Debug)]

pub struct ACKBody {
    pub RefSeqNum: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NAKBody {
    pub RefSeqNum: u32,
//    ErrorID: u32,
    pub Text: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewOrderBody{
    pub ClOrdID: String,        // Order identifier. Created by the combinantion of SeqNum and CustodyID
    pub Instrument: String,
    //Instrument: Instrument, // All instrument parameters
    pub Side: String,           // "1" = BUY, "2" = SELL
    pub Price: String,          // Limit price
//    TransactTime: u64,      // Time this order request was initiated
    pub OrderQtyData: String,
    pub OrdType: String,        // "1" = MARKET, "2" = LIMIT
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ExecReportBody {
    ClOrdID: String,            // Order identifier
//    ExecSeqNum: String,         // Execution identifier
    ExecType: String,           // "0" = NEW, "4" = CANCELED, "F" = TRADE, "B" = EXPIRED
    OrdStatus: String,          // "0" = NEW, "1" = PARTIALLY FILLED, "2" = FILLED, "4" = CANCELED, "8" = REJECTED
    Side: String,               // "1" = BUY, "2" = SELL
//    TransactTime: u64,          // Time this order request was initiated
    OrderQtyData: String,
    Price: String,              // Execution price
    CumQty: String,             // Cumulative executed quantity
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Body {
    ACKBody {
        RefSeqNum: u32,
    },
    NAKBody {
        RefSeqNum: u32,
        //    ErrorID: u32,
        Text: String,
    },
    NewOrderBody{
        ClOrdID: String,        // Order identifier. Created by the combinantion of SeqNum and CustodyID
        Instrument: String,
        //Instrument: Instrument, // All instrument parameters
        Side: String,           // "1" = BUY, "2" = SELL
        Price: String,          // Limit price
        //    TransactTime: u64,      // Time this order request was initiated
        OrderQtyData: String,
        OrdType: String,        // "1" = MARKET, "2" = LIMIT
    },
    NewIndexOrderBody{
        ClOrdID: String,        // Order identifier. Created by the combinantion of SeqNum and CustodyID
        Symbol: String,
        Side: String,           // "1" = BUY, "2" = SELL
        timestamp: u64,      // Time this order request was initiated
        Amount: String,
    },
    ExecReportBody {
        ClOrdID: String,            // Order identifier
    //    ExecSeqNum: String,         // Execution identifier
        ExecType: String,           // "0" = NEW, "4" = CANCELED, "F" = TRADE, "B" = EXPIRED
        OrdStatus: String,          // "0" = NEW, "1" = PARTIALLY FILLED, "2" = FILLED, "4" = CANCELED, "8" = REJECTED
        Side: String,               // "1" = BUY, "2" = SELL
    //    TransactTime: u64,          // Time this order request was initiated
        OrderQtyData: String,
        Price: String,              // Execution price
        CumQty: String,             // Cumulative executed quantity
    }
}
