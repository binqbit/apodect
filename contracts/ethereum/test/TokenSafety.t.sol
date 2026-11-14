// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { ProjectVaultTestBase } from "./support/ProjectVaultTestBase.sol";
import { ProjectVault } from "../src/ProjectVault.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { TestUSDC, FeeToken, ReentrantToken } from "./TestAssets.sol";

contract TokenSafetyTest is ProjectVaultTestBase {
    function testTransferFailureRollsBackFirstUseAndAllowsCancellation() public {
        (uint256 taskId,) = _funded(10);
        token.setFailTransfers(true);

        vm.expectRevert(
            abi.encodeWithSelector(SafeERC20.SafeERC20FailedOperation.selector, address(token))
        );
        vm.prank(agent);
        vault.withdraw(taskId, 5);

        assertFalse(vault.getTask(taskId).started);
        assertEq(vault.getTask(taskId).available, 10);
        assertEq(vault.allocatedBalance(), 10);

        vm.prank(owner);
        vault.cancel(taskId);
    }

    function testDonationsAndWrongTokensDoNotCreditProjectOrTask() public {
        TestUSDC wrong = new TestUSDC();
        wrong.mint(address(vault), 100);
        token.mint(address(vault), 100);
        (uint256 taskId, uint256 requestId) = _create(agent, 10);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Waiting);

        assertEq(vault.accountedBalance(), 0);
        assertEq(vault.getTask(taskId).available, 0);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(owner);
        vault.withdrawFree(1);
    }

    function testExternalReturnFailureDoesNotCreditRecoveredOrFreePrincipal() public {
        (uint256 taskId,) = _funded(10);

        vm.prank(agent);
        vault.withdraw(taskId, 10);
        token.setFailTransfers(true);

        vm.expectRevert(
            abi.encodeWithSelector(SafeERC20.SafeERC20FailedOperation.selector, address(token))
        );
        vm.prank(agent);
        vault.returnFunds(taskId, 5);

        assertEq(vault.getTask(taskId).recovered, 0);
        assertEq(vault.externalOutstanding(taskId), 10);
        assertEq(vault.freeBalance(), 0);
        assertEq(token.balanceOf(agent), 10);
    }

    function testOwnerWithdrawalFailureKeepsFreePrincipalAvailableForOneRetry() public {
        _fund(20);
        uint256 ownerBefore = token.balanceOf(owner);
        token.setFailTransfers(true);

        vm.expectRevert(
            abi.encodeWithSelector(SafeERC20.SafeERC20FailedOperation.selector, address(token))
        );
        vm.prank(owner);
        vault.withdrawFree(20);

        assertEq(vault.freeBalance(), 20);
        assertEq(token.balanceOf(address(vault)), 20);
        assertEq(token.balanceOf(owner), ownerBefore);

        token.setFailTransfers(false);
        vm.prank(owner);
        vault.withdrawFree(20);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(owner);
        vault.withdrawFree(20);

        assertEq(vault.freeBalance(), 0);
        assertEq(token.balanceOf(address(vault)), 0);
        assertEq(token.balanceOf(owner), ownerBefore + 20);
    }

    function testIncomingAndOutgoingTransferFeesAreRejectedAtomically() public {
        FeeToken feeToken = new FeeToken();
        ProjectVault feeVault = new ProjectVault(address(feeToken), owner, authority);
        feeToken.mint(owner, 100);

        vm.prank(owner);
        feeToken.approve(address(feeVault), 100);
        feeToken.setFeeEnabled(true);

        vm.expectRevert(ProjectDefinitions.NonExactTransfer.selector);
        vm.prank(owner);
        feeVault.fund(100, 0);

        assertEq(feeToken.balanceOf(owner), 100);
        assertEq(feeVault.freeBalance(), 0);

        feeToken.setFeeEnabled(false);

        vm.prank(owner);
        feeVault.fund(100, 0);

        vm.prank(agent);
        (uint256 taskId, uint256 requestId) = feeVault.createTask(100, PURPOSE);

        vm.prank(authority);
        feeVault.approveRequest(requestId);

        vm.prank(authority);
        feeVault.applyNext();
        feeToken.setFeeEnabled(true);

        vm.expectRevert(ProjectDefinitions.NonExactTransfer.selector);
        vm.prank(agent);
        feeVault.withdraw(taskId, 50);

        assertFalse(feeVault.getTask(taskId).started);
        assertEq(feeVault.getTask(taskId).available, 100);
        assertEq(feeToken.balanceOf(address(feeVault)), 100);
    }

    function testTokenCannotReenterAnAuthenticatedAgentWithdrawal() public {
        ReentrantToken malicious = new ReentrantToken();
        ProjectVault guarded = new ProjectVault(address(malicious), owner, authority);
        malicious.mint(owner, 100);

        vm.startPrank(owner);
        malicious.approve(address(guarded), 100);
        guarded.fund(100, 0);
        vm.stopPrank();

        (uint256 taskId, uint256 requestId) = malicious.createOwnTask(guarded, 100);

        vm.startPrank(authority);
        guarded.approveRequest(requestId);
        guarded.applyNext();
        vm.stopPrank();

        malicious.beginWithdrawal(guarded, taskId, 30);

        assertTrue(malicious.attempted());
        assertFalse(malicious.reentrySucceeded());
        assertEq(guarded.getTask(taskId).available, 70);
        assertEq(malicious.balanceOf(address(malicious)), 30);
        assertEq(guarded.externalOutstanding(taskId), 30);
    }

    function testSelfRecipientCannotManufactureAnExternalWithdrawal() public {
        _fund(10);

        // There is no real self-call entry point. Impersonation exercises the exact-transfer
        // guard as an additional defense against a hypothetical self-recipient integration.
        (uint256 taskId, uint256 requestId) = _create(address(vault), 10);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        vm.expectRevert(ProjectDefinitions.NonExactTransfer.selector);
        vm.prank(address(vault));
        vault.withdraw(taskId, 10);

        assertEq(vault.getTask(taskId).available, 10);
        assertFalse(vault.getTask(taskId).started);
        assertEq(vault.externalOutstanding(taskId), 0);
    }
}
