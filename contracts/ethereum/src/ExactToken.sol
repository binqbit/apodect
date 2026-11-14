// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ProjectDefinitions as Definitions } from "./ProjectDefinitions.sol";

/// @notice Exact principal movement for a vault holding a non-rebasing ERC-20.
/// @dev Internal functions execute in the vault; no library address or approval is required.
library ExactToken {
    using SafeERC20 for IERC20;

    function receiveExact(IERC20 asset, address from, uint256 amount) internal {
        uint256 vaultBefore = asset.balanceOf(address(this));
        uint256 senderBefore = asset.balanceOf(from);
        if (senderBefore < amount) revert Definitions.InsufficientFunds();

        asset.safeTransferFrom(from, address(this), amount);

        if (
            asset.balanceOf(address(this)) != vaultBefore + amount
                || asset.balanceOf(from) != senderBefore - amount
        ) revert Definitions.NonExactTransfer();
    }

    function sendExact(IERC20 asset, address to, uint256 amount) internal {
        uint256 vaultBefore = asset.balanceOf(address(this));
        uint256 recipientBefore = asset.balanceOf(to);
        if (vaultBefore < amount) revert Definitions.InsufficientFunds();

        asset.safeTransfer(to, amount);

        if (
            asset.balanceOf(address(this)) != vaultBefore - amount
                || asset.balanceOf(to) != recipientBefore + amount
        ) revert Definitions.NonExactTransfer();
    }
}
