use alloy::sol;

sol! {
    #[sol(rpc)]
    contract AcrossConnector {
        function deposit(
            address recipient,
            address inputToken,
            address outputToken,
            uint256 amount,
            uint256 minAmount,
            uint256 destinationChainId,
            address exclusiveRelayer,
            uint32 fillDeadline,
            uint32 exclusivityDeadline,
            bytes calldata message
        ) external;

        function spokePool() external view returns (address);

        function setTargetChainMulticallHandler(uint256 chainId, address handler) external;
    }

    #[sol(rpc)]
    contract ERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
    }

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
