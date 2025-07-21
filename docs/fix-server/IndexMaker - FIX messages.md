# PROTOCOL

The messages exchanged by the fix-server are of the fix-json type, using fix protocol logic in a json format and with own message fields. Messages must be sent in sequence, with an increasing `seq_num`. Sequence numbers out-of-order will result in the message being considered invalid (`NAK`). A message that receives a `NAK` as response does not increase the `seq_num`. Both parties must track their sent and received sequence numbers. Messages which fields do not match their declared `msg_type` will also be responded with a `NAK`.

If the message is considered valid, an `ACK` response will be sent. This means the message is valid on the protocol scope, a message can still be rejected after receiving an `ACK` given the processing logic of the quote server. E.g. a quote request for a nonexistent symbol can be `ACK`'d due to the message being protocol compliant, but later rejected for the symbol is not found.


## MESSAGE STRUCTURE

The general message structure is the following:

```
struct FixRequest {
    standard_header: FixHeader,
    chain_id: u32,
    address: Address,

    <aditional fields> // these fileds change according to the message type

    standard_trailer: FixTrailer,
}
```

Where

```
struct FixHeader {
    msg_type: String,        // Type of message, e.g. "NewIndexOrder"
    sender_comp_id: String,
    target_comp_id: String,
    seq_num: u32,
    timestamp: DateTime<Utc>,
}
```

and

```
struct FixTrailer {
    public_key: Vec<String>,
    signature: Vec<String>,
}
```
.

Thus a full message will have the following structure:


```
struct FixRequest {
    standard_header: {
        msg_type: String, 
        sender_comp_id: String,
        target_comp_id: String,
        seq_num: u32,
        timestamp: DateTime<Utc>,
    },
    chain_id: u32,
    address: Address,

    additional_field_0: String,
    additional_field_1: u32,
    ...

    standard_trailer: {
        public_key: Vec<String>,
        signature: Vec<String>,
    },
}
```

This structure is used for both client's requests and server's responses. The following section will differentiate messages according to their types and additional fields.



## CLIENT'S REQUESTS

### NewQuoteRequest
```
client_quote_id: String,
symbol: String,
side: String,
amount: String,
```

Responded with **NewIndexQuote** to notify quote acceptance or rejection and **IndexQuoteResponse** for quote value.


### CancelQuoteRequest
```
client_quote_id: String,
symbol: String,
```

Responded with **CancelIndexQuote**.


### NewIndexOrder (unavailable on quote server)
```
client_order_id: String,
symbol: String,
side: String,
amount: String,
```

Responded with **NewIndexOrder** to notify order acceptance or rejection, one or more **IndexOrderFill** for the fills of the order, and a final **MintInvoice** to acknowledge the minting of the index.


### CancelIndexOrderBody (unavailable on quote server)
```
client_order_id: String,
symbol: String,
amount: String,
```

Responded with **CancelIndexOrder**.


## SERVER'S RESPONSES

### ACK
```
ref_seq_num: u32,
```


### NAK
```
ref_seq_num: u32,
reason: String,
```


### NewIndexQuote
```
status: String,
client_quote_id: String,
reason: String, (only on `rejected` status)
```

Status can be either *new* or *rejected*.


### CancelIndexQuote
```
status: String,
client_quote_id: String,
reason: String, (only on `rejected` status)
```

Status can be either *canceled* or *rejected*.


### IndexQuoteResponse
```
client_quote_id: String,
quantity_possible: String,
```


### NewIndexOrder
```
status: String,
client_order_id: String,
reason: String, (only on `rejected` status)
```

Status can be either *new* or *rejected*.



### CancelIndexOrder
```
status: String,
client_order_id: String,
reason: String, (only on `rejected` status)
```

Status can be either *canceled* or *rejected*.


### IndexOrderFill
```
client_order_id: String,
filled_quantity: String,
collateral_spent: String,
collateral_remaining: String,
```


### MintInvoice
```
timestamp: DateTime<Utc>,
order_id: String,
index_id: String,
collateral_spent: String,
total_collateral: String,
engaged_collateral: String,
management_fee: String,
payment_id: String,
amount_paid: String,
lots: Vec<SolverOrderAssetLot>,
```
