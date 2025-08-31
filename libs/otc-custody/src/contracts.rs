use alloy::sol;

sol! {
    #[sol(rpc)]
    contract ERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function decimals() external view returns (uint8);
        function name() external view returns (string memory);
        function symbol() external view returns (string memory);
    }

    #[sol(rpc)]
    contract OTCIndex {
        function debugDeployDigest(
            uint256 ts,
            bytes32 id,
            string calldata connectorType,
            address factory,
            bytes calldata data
        ) external view returns (bytes32) {
            // Call the SAME internal routine your verifier uses.
            // e.g., return VerificationUtils.deployConnectorDigest(...);
        }
        function solverUpdate(uint256 _timestamp, bytes memory _weights, uint256 _price) external;
        function mint(address target, uint256 amount, uint256 seqNumExecutionReport) external;
        function burn(uint256 amount, address target, uint256 seqNumNewOrderSingle) external;
        function withdraw(uint256 amount, address to, VerificationData memory v, bytes calldata executionReport) external;
    }

    #[sol(rpc)]
    interface IOTCIndex {
        event Deposit(
            uint256 amount,
            address from,
            uint256 seqNumNewOrderSingle,
            address affiliate1,
            address affiliate2
        );
        event Mint(
            uint256 amount,
            address to,
            uint256 seqNumExecutionReport
        );
        event Withdraw(uint256 amount, address to, bytes executionReport);
        function getCollateralToken() external view returns (address);
        function mint(address target, uint256 amount, uint256 seqNumExecutionReport) external;
    }

    #[sol(rpc)]
    contract IndexFactory {
        event IndexDeployed(address indexed indexAddress);

        function deployConnector(
            bytes32 custodyId,
            bytes calldata data,
            address _whitelistedCaller
        ) external returns (address);

        function deployIndex(
            string memory _name,
            string memory _symbol,
            bytes32 _custodyId,
            address _collateralToken,
            uint256 _collateralTokenPrecision,
            uint256 _managementFee,
            uint256 _performanceFee,
            uint256 _maxMintPerBlock,
            uint256 _maxRedeemPerBlock,
            uint256 _voteThreshold,
            uint256 _votePeriod,
            uint256 _initialPrice
        ) external returns (address);
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
        function deployConnector(
            string calldata _connectorType,
            address _factoryAddress,
            bytes calldata _data,
            VerificationData calldata v
        ) external;
        function isConnectorWhitelisted(address connector) external view returns (bool);
        function getCustodyBalances(bytes32 id, address token) external view returns (uint256);
        function getCustodyState(bytes32 id) external view returns (uint8);
        function getCA(bytes32 id) external view returns (bytes32);
        function getCustodyOwner(bytes32 id) external view returns (address);
    }

    struct SchnorrCAKey {
        uint8 parity;
        bytes32 x;
    }

    struct SchnorrSignature {
        bytes32 e;
        bytes32 s;
    }

    struct VerificationData {
        bytes32 id;
        uint8 state;
        uint256 timestamp;
        SchnorrCAKey pubKey;
        SchnorrSignature sig;
        bytes32[] merkleProof;
    }
}
