// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { ProjectVaultTestBase } from "./support/ProjectVaultTestBase.sol";
import { ProjectVault } from "../src/ProjectVault.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";
import { TestUSDC } from "./TestAssets.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

contract ProjectAccountingTest is ProjectVaultTestBase {
    function testAccountingExample() public {
        _fund(100);
        (uint256 taskId, uint256 requestId) = _create(agent, 60);
        _approve(requestId);

        assertEq(vault.freeBalance(), 100);

        _apply(ProjectDefinitions.ApplyResult.Funded);

        vm.prank(agent);
        vault.withdraw(taskId, 40);

        vm.prank(agent);
        vault.returnFunds(taskId, 15);

        assertEq(vault.getTask(taskId).available, 20);
        assertEq(vault.externalOutstanding(taskId), 25);

        vm.prank(agent);
        vault.release(taskId, 20);

        vm.prank(agent);
        vault.finalize(taskId, keccak256("report"));

        assertEq(vault.taskExpense(taskId), 25);
        assertEq(vault.freeBalance(), 75);
        assertEq(vault.allocatedBalance(), 0);
        assertEq(token.balanceOf(address(vault)), 75);
        assertEq(vault.getTask(taskId).issued, 60);
        assertEq(vault.getTask(taskId).recovered, 35);
    }

    function testReturnAndControlledReleaseHaveDifferentDeltas() public {
        (uint256 taskId,) = _funded(100);

        vm.prank(agent);
        vault.withdraw(taskId, 60);

        vm.prank(agent);
        vault.returnFunds(taskId, 30);

        assertEq(vault.externalOutstanding(taskId), 30);
        assertEq(vault.getTask(taskId).available, 40);
        assertEq(vault.freeBalance(), 30);

        vm.prank(agent);
        vault.release(taskId, 30);

        assertEq(vault.externalOutstanding(taskId), 30);
        assertEq(vault.getTask(taskId).available, 10);
        assertEq(vault.freeBalance(), 60);

        vm.expectRevert(ProjectDefinitions.ExcessReturn.selector);
        vm.prank(agent);
        vault.returnFunds(taskId, 31);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 11);
    }

    function testReturnedPrincipalFundsAnotherAgentWithoutRefillingOldAllowance() public {
        (uint256 firstTask,) = _funded(50);

        vm.prank(agent);
        vault.withdraw(firstTask, 30);

        (uint256 nextTask, uint256 nextRequest) = _create(other, 15);
        _approve(nextRequest);
        _apply(ProjectDefinitions.ApplyResult.Waiting);

        vm.prank(agent);
        vault.returnFunds(firstTask, 15);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertEq(vault.getTask(firstTask).available, 20);
        assertEq(vault.getTask(nextTask).available, 15);
        assertEq(vault.freeBalance(), 0);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(agent);
        vault.withdraw(firstTask, 21);

        vm.prank(other);
        vault.withdraw(nextTask, 15);
        vm.prank(agent);
        vault.withdraw(firstTask, 20);

        assertEq(vault.accountedBalance(), 0);
        assertEq(vault.externalOutstanding(firstTask), 35);
        assertEq(vault.externalOutstanding(nextTask), 15);
        assertEq(token.balanceOf(agent) + token.balanceOf(other), 50);
    }

    function testZeroExcessAndUnknownValuesRevert() public {
        vm.expectRevert(ProjectDefinitions.InvalidAmount.selector);
        vm.prank(owner);
        vault.fund(0, 0);

        vm.expectRevert(ProjectDefinitions.InvalidAmount.selector);
        vault.createTask(0, PURPOSE);

        vm.expectRevert(ProjectDefinitions.MissingPurpose.selector);
        vault.createTask(1, bytes32(0));

        vm.expectRevert(ProjectDefinitions.UnknownTask.selector);
        vault.getTask(0);

        vm.expectRevert(ProjectDefinitions.UnknownRequest.selector);
        vault.getRequest(1);

        vm.expectRevert(ProjectDefinitions.UnknownInvoice.selector);
        vault.getInvoice(1);
        (uint256 taskId,) = _funded(1);

        vm.expectRevert(ProjectDefinitions.InvalidAmount.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 0);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(agent);
        vault.withdraw(taskId, type(uint256).max);

        vm.expectRevert(ProjectDefinitions.InvalidAmount.selector);
        vm.prank(agent);
        vault.release(taskId, 0);

        vm.expectRevert(ProjectDefinitions.InvalidAddress.selector);
        vm.prank(owner);
        vault.setAuthority(address(0));
    }

    function testCumulativeCounterOverflowRevertsWithoutReusingFunds() public {
        TestUSDC maximumToken = new TestUSDC();
        ProjectVault maximumVault = new ProjectVault(address(maximumToken), owner, authority);
        maximumToken.mint(owner, type(uint256).max);

        vm.startPrank(owner);
        maximumToken.approve(address(maximumVault), type(uint256).max);
        maximumVault.fund(type(uint256).max, 0);
        vm.stopPrank();

        vm.prank(agent);
        (uint256 taskId, uint256 requestId) = maximumVault.createTask(type(uint256).max, PURPOSE);

        vm.startPrank(authority);
        maximumVault.approveRequest(requestId);
        maximumVault.applyNext();
        vm.stopPrank();

        vm.startPrank(agent);
        maximumVault.withdraw(taskId, 1);
        maximumToken.approve(address(maximumVault), 1);
        maximumVault.returnFunds(taskId, 1);
        uint256 supplement = maximumVault.requestBudget(taskId, 1, PURPOSE);
        vm.stopPrank();

        vm.prank(authority);
        maximumVault.approveRequest(supplement);

        vm.expectRevert(abi.encodeWithSignature("Panic(uint256)", 0x11));
        vm.prank(authority);
        maximumVault.applyNext();

        assertEq(maximumVault.freeBalance(), 1);
        assertEq(maximumVault.allocatedBalance(), type(uint256).max - 1);
        assertEq(maximumVault.getTask(taskId).issued, type(uint256).max);
        assertEq(maximumVault.getTask(taskId).recovered, 1);
        assertEq(maximumVault.getTask(taskId).liveRequestId, supplement);
        assertEq(maximumVault.queueHead(), 1);
        assertEq(maximumToken.balanceOf(address(maximumVault)), type(uint256).max);
    }

    function testFuzzConservationAcrossReturnAndRelease(
        uint128 allocation,
        uint128 withdrawal,
        uint128 returned
    ) public {
        uint256 amount = bound(uint256(allocation), 1, 1e18);
        uint256 withdrawn = bound(uint256(withdrawal), 1, amount);
        uint256 recovered = bound(uint256(returned), 0, withdrawn);
        (uint256 taskId,) = _funded(amount);

        vm.prank(agent);
        vault.withdraw(taskId, withdrawn);
        if (recovered != 0) {
            vm.prank(agent);
            vault.returnFunds(taskId, recovered);
        }
        if (amount != withdrawn) {
            vm.prank(agent);
            vault.release(taskId, amount - withdrawn);
        }

        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        assertEq(vault.taskExpense(taskId), withdrawn - recovered);
        assertEq(vault.freeBalance() + vault.taskExpense(taskId), amount);
        assertEq(vault.accountedBalance(), token.balanceOf(address(vault)));
    }

    function testInvoicePartialAndZeroResponseCloseWithoutCashAndFundingIsSeparate() public {
        vm.startPrank(authority);
        uint256 partialInvoice = vault.requestReplenishment(100, PURPOSE);
        uint256 zero = vault.requestReplenishment(100, bytes32(0));
        vm.stopPrank();

        vm.expectRevert(ProjectDefinitions.InvalidInvoiceState.selector);
        vm.prank(owner);
        vault.fund(10, partialInvoice);

        vm.startPrank(owner);
        vault.respondReplenishment(partialInvoice, 60, PURPOSE);
        vault.respondReplenishment(zero, 0, bytes32(0));
        vm.stopPrank();

        assertEq(vault.freeBalance(), 0);
        assertEq(vault.getInvoice(partialInvoice).intendedContribution, 60);
        assertEq(
            uint256(vault.getInvoice(zero).status), uint256(ProjectDefinitions.InvoiceStatus.Closed)
        );

        vm.expectRevert(ProjectDefinitions.InvalidInvoiceState.selector);
        vm.prank(owner);
        vault.respondReplenishment(partialInvoice, 100, bytes32(0));
        token.setFailTransfers(true);

        vm.expectRevert(
            abi.encodeWithSelector(SafeERC20.SafeERC20FailedOperation.selector, address(token))
        );
        vm.prank(owner);
        vault.fund(60, partialInvoice);

        assertEq(vault.getInvoice(partialInvoice).funded, 0);
        assertEq(
            uint256(vault.getInvoice(partialInvoice).status),
            uint256(ProjectDefinitions.InvoiceStatus.Closed)
        );

        token.setFailTransfers(false);

        vm.prank(owner);
        vault.fund(60, partialInvoice);

        assertEq(vault.getInvoice(partialInvoice).funded, 60);
        assertEq(vault.freeBalance(), 60);
    }
}
