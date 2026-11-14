// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { ProjectVaultTestBase } from "./support/ProjectVaultTestBase.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";

contract TaskLifecycleTest is ProjectVaultTestBase {
    function testOwnerPauseSurvivesStoppedTaskResumption() public {
        (uint256 taskId,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(taskId, 5);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(taskId, 10, PURPOSE);

        vm.prank(authority);
        vault.rejectRequest(supplement);

        vm.prank(owner);
        vault.pause(taskId);

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(owner);
        vault.continueTask(taskId);

        vm.prank(agent);
        uint256 resume = vault.requestBudget(taskId, 10, PURPOSE);
        _approve(resume);
        _fund(10);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertTrue(vault.getTask(taskId).paused);

        vm.expectRevert(ProjectDefinitions.TaskIsPaused.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(owner);
        vault.continueTask(taskId);
    }

    function testOrdinaryCancelClosesUnusedAllocationAndOwnerCanWithdraw() public {
        (uint256 taskId,) = _funded(50);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(owner);
        vault.withdrawFree(1);

        vm.prank(owner);
        vault.cancel(taskId);

        assertEq(vault.getTask(taskId).available, 0);
        assertEq(vault.getTask(taskId).recovered, 50);
        assertEq(vault.freeBalance(), 50);

        vm.prank(owner);
        vault.withdrawFree(50);

        assertEq(vault.accountedBalance(), 0);

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);
    }

    function testOrdinaryCancelLosesAfterFirstWithdrawalEvenAfterReturn() public {
        (uint256 taskId,) = _funded(10);

        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(agent);
        vault.returnFunds(taskId, 1);

        vm.expectRevert(ProjectDefinitions.TaskAlreadyStarted.selector);
        vm.prank(owner);
        vault.cancel(taskId);

        assertTrue(vault.getTask(taskId).started);
    }

    function testPauseBlocksSpendAllowsReturnAndRetainedContinueDoesNotIssue() public {
        (uint256 taskId,) = _funded(30);

        vm.prank(agent);
        vault.withdraw(taskId, 10);

        vm.prank(owner);
        vault.pause(taskId);

        assertEq(vault.freeBalance(), 0);

        vm.expectRevert(ProjectDefinitions.TaskIsPaused.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(agent);
        vault.returnFunds(taskId, 5);

        assertEq(vault.freeBalance(), 5);
        assertEq(vault.getTask(taskId).available, 20);

        vm.prank(owner);
        vault.continueTask(taskId);

        assertEq(vault.getTask(taskId).issued, 30);
        assertEq(vault.getTask(taskId).recovered, 5);
        assertTrue(vault.getTask(taskId).started);
    }

    function testReclaimRestoreUsesFIFOAndDoesNotInflateExpense() public {
        (uint256 taskId,) = _funded(60);

        vm.prank(agent);
        vault.withdraw(taskId, 20);

        vm.prank(owner);
        vault.pause(taskId);

        vm.prank(owner);
        vault.reclaim(taskId, 40);

        assertEq(vault.freeBalance(), 40);

        (, uint256 earlier) = _create(other, 50);
        _approve(earlier);

        vm.prank(authority);
        uint256 restoration = vault.restore(taskId, 40, PURPOSE);

        assertEq(
            uint256(vault.getRequest(restoration).status),
            uint256(ProjectDefinitions.RequestStatus.Queued)
        );

        _apply(ProjectDefinitions.ApplyResult.Waiting);

        assertEq(vault.getTask(taskId).available, 0);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(owner);
        vault.continueTask(taskId);
        _fund(50);
        _apply(ProjectDefinitions.ApplyResult.Funded);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertTrue(vault.getTask(taskId).paused);

        vm.expectRevert(ProjectDefinitions.Unauthorized.selector);
        vm.prank(authority);
        vault.continueTask(taskId);

        vm.prank(owner);
        vault.continueTask(taskId);

        vm.prank(agent);
        vault.release(taskId, 40);

        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        assertEq(vault.getTask(taskId).issued, 100);
        assertEq(vault.getTask(taskId).recovered, 80);
        assertEq(vault.taskExpense(taskId), 20);
    }

    function testReclaimCannotTakeExternalFunds() public {
        (uint256 taskId,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(taskId, 15);

        vm.prank(owner);
        vault.pause(taskId);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(owner);
        vault.reclaim(taskId, 6);

        assertEq(token.balanceOf(agent), 15);
        assertEq(vault.getTask(taskId).available, 5);
    }

    function testRestoreRequiresPreviouslyFundedPausedActiveTaskAndNoLiveRequest() public {
        (uint256 taskId, uint256 requestId) = _create(agent, 10);

        vm.prank(owner);
        vault.pause(taskId);

        vm.expectRevert(ProjectDefinitions.TaskNotFunded.selector);
        vm.prank(authority);
        vault.restore(taskId, 10, PURPOSE);
        _fund(10);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        vm.prank(authority);
        vault.restore(taskId, 1, PURPOSE);

        vm.expectRevert(ProjectDefinitions.LiveRequestExists.selector);
        vm.prank(authority);
        vault.restore(taskId, 1, PURPOSE);
    }

    function testOutOfBudgetRetainsAllocationAllowsCleanupAndCannotReopen() public {
        (uint256 taskId,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(taskId, 5);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(taskId, 10, PURPOSE);
        _approve(supplement);

        vm.prank(authority);
        vault.markOutOfBudget(taskId);

        assertEq(vault.freeBalance(), 0);
        assertEq(vault.getTask(taskId).available, 15);
        assertEq(
            uint256(vault.getRequest(supplement).status),
            uint256(ProjectDefinitions.RequestStatus.Removed)
        );

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(agent);
        vault.requestBudget(taskId, 1, PURPOSE);

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(agent);
        vault.returnFunds(taskId, 5);

        vm.prank(agent);
        vault.release(taskId, 15);

        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        assertEq(vault.taskExpense(taskId), 0);

        _create(agent, 20);
    }

    function testFinalizeRequiresFundedAndExplicitRelease() public {
        (uint256 taskId, uint256 requestId) = _create(agent, 10);

        vm.expectRevert(ProjectDefinitions.TaskNotFunded.selector);
        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        _fund(10);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        vm.expectRevert(ProjectDefinitions.ControlledRemainder.selector);
        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        vm.prank(agent);
        vault.release(taskId, 10);

        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        assertEq(vault.taskExpense(taskId), 0);
    }

    function testFinalizeAtomicallyRemovesPendingOrQueuedSupplement() public {
        for (uint256 queued; queued < 2; ++queued) {
            (uint256 taskId,) = _funded(10);

            vm.prank(agent);
            vault.withdraw(taskId, 10);

            vm.prank(agent);
            uint256 supplement = vault.requestBudget(taskId, 20, PURPOSE);
            if (queued == 1) _approve(supplement);

            vm.prank(agent);
            vault.finalize(taskId, PURPOSE);

            assertEq(vault.getTask(taskId).liveRequestId, 0);
            assertEq(
                uint256(vault.getRequest(supplement).status),
                uint256(ProjectDefinitions.RequestStatus.Removed)
            );

            if (queued == 1) _apply(ProjectDefinitions.ApplyResult.Skipped);

            vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
            vm.prank(agent);
            vault.returnFunds(taskId, 1);

            vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
            vm.prank(agent);
            vault.finalize(taskId, bytes32(0));

            assertEq(vault.taskExpense(taskId), 10);
        }
    }

    function testPausedStoppedTaskCanFinalizeAfterCleanup() public {
        (uint256 taskId,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(taskId, 5);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(taskId, 1, PURPOSE);

        vm.prank(authority);
        vault.rejectRequest(supplement);

        vm.prank(owner);
        vault.pause(taskId);

        vm.prank(agent);
        vault.release(taskId, 15);

        vm.prank(agent);
        vault.finalize(taskId, bytes32(0));

        assertEq(vault.taskExpense(taskId), 5);
    }

    function testStopEventsRecordTheReasonSeparately() public {
        (uint256 taskId,) = _funded(10);

        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(taskId, 5, PURPOSE);

        vm.expectEmit(true, false, false, true, address(vault));
        emit ProjectDefinitions.TaskStopped(
            taskId, ProjectDefinitions.StopReason.SupplementRejected
        );
        vm.prank(authority);
        vault.rejectRequest(supplement);

        vm.expectEmit(true, false, false, true, address(vault));
        emit ProjectDefinitions.TaskStopped(
            taskId, ProjectDefinitions.StopReason.AuthorityOutOfBudget
        );
        vm.prank(authority);
        vault.markOutOfBudget(taskId);
    }
}
