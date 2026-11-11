// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { ExactToken } from "./ExactToken.sol";

/// @notice A route-specific receiver for plain ERC-20 delivery or refund transfers.
/// @dev Only its immutable Project may collect; excess unsolicited tokens remain here.
contract RouteEscrow {
    using ExactToken for IERC20;

    IERC20 public immutable asset;
    address public immutable project;

    error Unauthorized();

    constructor(IERC20 asset_) {
        asset = asset_;
        project = msg.sender;
    }

    function collect(uint256 amount) external {
        if (msg.sender != project) revert Unauthorized();
        asset.sendExact(project, amount);
    }
}
