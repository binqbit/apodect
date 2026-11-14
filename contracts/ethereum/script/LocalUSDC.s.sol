// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @notice Freely mintable six-decimal asset for the local Anvil environment only.
contract LocalUSDC is ERC20 {
    constructor() ERC20("Apodect Local USDC", "USDC") { }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }
}
