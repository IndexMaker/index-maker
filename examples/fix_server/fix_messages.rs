
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug)]
pub struct FixHeader {
    pub MsgType: String,       // Type of message, e.g. "NewOrderSingle"
    pub SenderCompID: String,  // Public key of sender
    pub TargetCompID: String,  // Public key of receiver
    pub SeqNum: u32,           // Message sequence number
//    ResetSeqNum: u8,       // Boolean that determines if SeqNum must be ignored (this will invalidate Disputes)
//    SendingTime: u64,      // Unix timestamp of sending time
//    CustodyId: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FixTrailer {
    pub PublicKey: Vec<String>,  // Public key of sender
    pub Signature: Vec<String>,  // Public key of receiver
//    RateLimitInterval: u32,           // Message sequence number
//    RateLimitCount: u32,
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