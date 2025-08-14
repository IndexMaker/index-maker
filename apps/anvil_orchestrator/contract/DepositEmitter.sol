// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice Minimal contract that only emits a Deposit event.
/// @dev Matches your FE shape: deposit(uint256,address,uint256)
contract DepositEmitter {
    event Deposit(address indexed account, uint256 chainId, uint256 amount);

    function deposit(uint256 chainId, address account, uint256 amount) external {
        emit Deposit(account, chainId, amount);
    }
}
