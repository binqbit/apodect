// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { ProjectVault } from "../src/ProjectVault.sol";

contract TestUSDC is ERC20 {
    bool public failTransfers;

    constructor() ERC20("Local Test USDC", "USDC") { }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function setFailTransfers(bool fail) external {
        failTransfers = fail;
    }

    function transfer(address to, uint256 amount) public override returns (bool) {
        if (failTransfers) return false;

        return super.transfer(to, amount);
    }

    function transferFrom(address from, address to, uint256 amount) public override returns (bool) {
        if (failTransfers) return false;

        return super.transferFrom(from, to, amount);
    }
}

contract WrongDecimalsToken is ERC20 {
    constructor() ERC20("Wrong", "WRONG") { }
}

contract FeeToken is ERC20 {
    bool public feeEnabled;

    constructor() ERC20("Fee", "FEE") { }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function setFeeEnabled(bool enabled) external {
        feeEnabled = enabled;
    }

    function _update(address from, address to, uint256 amount) internal override {
        if (feeEnabled && from != address(0) && to != address(0) && amount != 0) {
            super._update(from, address(0), 1);
            super._update(from, to, amount - 1);
        } else {
            super._update(from, to, amount);
        }
    }
}

contract ReentrantToken is ERC20 {
    ProjectVault private _vault;
    uint256 private _taskId;
    uint256 private _amount;
    bool private _armed;
    bool public attempted;
    bool public reentrySucceeded;

    constructor() ERC20("Reentrant", "RNT") { }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function createOwnTask(ProjectVault vault, uint256 amount)
        external
        returns (uint256 taskId, uint256 requestId)
    {
        return vault.createTask(amount, keccak256("reentrancy test"));
    }

    function beginWithdrawal(ProjectVault vault, uint256 taskId, uint256 amount) external {
        _vault = vault;
        _taskId = taskId;
        _amount = amount;
        _armed = true;

        vault.withdraw(taskId, amount);
    }

    function transfer(address to, uint256 amount) public override returns (bool) {
        if (_armed) {
            _armed = false;
            attempted = true;
            (reentrySucceeded,) =
                address(_vault).call(abi.encodeCall(ProjectVault.withdraw, (_taskId, _amount)));
        }

        return super.transfer(to, amount);
    }
}
