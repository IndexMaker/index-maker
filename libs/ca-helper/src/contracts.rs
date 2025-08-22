use alloy::sol;

sol! {
    #[sol(rpc)]
    contract OTCCustody {
        function addressToCustody(bytes32 id, address token, uint256 amount) external;
        function custodyToAddress(address token, address destination, uint256 amount, VerificationData calldata v) external;
        function custodyToConnector(address token, address connectorAddress, uint256 amount, VerificationData calldata v) external;
        function callConnector(
            string calldata connectorType,
            address connectorAddress,
            bytes calldata fixedCallData,
            bytes calldata tailCallData,
            VerificationData calldata v
        ) external;
        function getCustodyBalances(bytes32 id, address token) external view returns (uint256);
        function getCustodyState(bytes32 id) external view returns (uint8);
        function getCA(bytes32 id) external view returns (bytes32);
    }

    struct VerificationData {
        bytes32 id;
        uint8 state;
        uint256 timestamp;
        CAKey pubKey;
        Signature sig;
        bytes32[] merkleProof;
    }

    struct CAKey {
        uint8 parity;
        bytes32 x;
    }

    struct Signature {
        bytes32 e;
        bytes32 s;
    }
}
