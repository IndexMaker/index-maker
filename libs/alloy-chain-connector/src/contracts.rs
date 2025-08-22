use alloy::sol;

sol! {
    #[sol(rpc)]
    contract ERC20 {
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function decimals() external view returns (uint8);
    }

    #[sol(rpc)]
    contract OTCIndex {
        function solverUpdate(uint256 _timestamp, bytes memory _weights, uint256 _price) external;
        function mint(address target, uint256 amount, uint256 seqNumExecutionReport) external;
        function burn(uint256 amount, address target, uint256 seqNumNewOrderSingle) external;
        function withdraw(uint256 amount, address to, VerificationData memory v, bytes calldata executionReport) external;
    }

    #[sol(rpc)]
    contract OTCCustody {
        function addressToCustody(bytes32 id, address token, uint256 amount) external;
        function custodyToAddress(address token, address destination, uint256 amount, VerificationData calldata v) external;
        function getCustodyBalances(bytes32 id,address token) external view returns (uint256);
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
