// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { ProjectVaultTestBase } from "./support/ProjectVaultTestBase.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";

contract BudgetRequestsTest is ProjectVaultTestBase {
    function testApprovalDoesNotReserveAndApplyAllocatesOnlyOnce() public {
        _fund(20);
        (uint256 taskId, uint256 requestId) = _create(agent, 20);
        _approve(requestId);

        assertEq(vault.freeBalance(), 20);
        assertEq(vault.getTask(taskId).available, 0);

        vm.expectRevert(ProjectDefinitions.InsufficientFunds.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);

        _apply(ProjectDefinitions.ApplyResult.Funded);
        _apply(ProjectDefinitions.ApplyResult.Empty);

        assertEq(vault.getTask(taskId).issued, 20);
        assertEq(vault.getTask(taskId).available, 20);

        vm.expectRevert(ProjectDefinitions.InvalidRequestState.selector);
        vm.prank(authority);
        vault.approveRequest(requestId);
    }

    function testApprovalOrderNotCreationOrderAndShortageCannotBeBypassed() public {
        _fund(30);
        (uint256 smallTask, uint256 smallRequest) = _create(agent, 10);
        (uint256 largeTask, uint256 largeRequest) = _create(other, 40);
        _approve(largeRequest);
        _approve(smallRequest);
        _apply(ProjectDefinitions.ApplyResult.Waiting);
        _apply(ProjectDefinitions.ApplyResult.Waiting);

        assertEq(vault.queueHead(), 0);
        assertEq(vault.getTask(smallTask).available, 0);
        assertEq(vault.getTask(largeTask).available, 0);
        assertEq(vault.freeBalance(), 30);

        _fund(20);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertEq(vault.getTask(largeTask).available, 40);
        assertEq(vault.getTask(smallTask).available, 0);

        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertEq(vault.getTask(smallTask).available, 10);
    }

    function testOneLiveRequestAndRejectedInitialReplacement() public {
        (uint256 taskId, uint256 first) = _create(agent, 10);

        vm.expectRevert(ProjectDefinitions.LiveRequestExists.selector);
        vm.prank(agent);
        vault.requestBudget(taskId, 20, PURPOSE);

        vm.prank(authority);
        vault.rejectRequest(first);

        assertEq(
            uint256(vault.getTask(taskId).status), uint256(ProjectDefinitions.TaskStatus.Active)
        );

        vm.prank(agent);
        uint256 next = vault.requestBudget(taskId, 20, PURPOSE);

        assertEq(
            uint256(vault.getRequest(next).kind), uint256(ProjectDefinitions.RequestKind.Initial)
        );

        _approve(next);

        vm.expectRevert(ProjectDefinitions.LiveRequestExists.selector);
        vm.prank(agent);
        vault.requestBudget(taskId, 30, PURPOSE);

        vm.expectRevert(ProjectDefinitions.InvalidRequestState.selector);
        vm.prank(authority);
        vault.rejectRequest(next);
    }

    function testSupplementNeedsFirstUse() public {
        (uint256 taskId,) = _funded(10);

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(agent);
        vault.requestBudget(taskId, 10, PURPOSE);
    }

    function testRejectedSupplementStopsAndNewFundedRequestResumes() public {
        (uint256 taskId,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(taskId, 5);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(taskId, 10, PURPOSE);

        vm.prank(authority);
        vault.rejectRequest(supplement);

        assertEq(
            uint256(vault.getTask(taskId).status), uint256(ProjectDefinitions.TaskStatus.Stopped)
        );
        assertEq(vault.getTask(taskId).available, 15);
        assertEq(vault.freeBalance(), 0);

        vm.expectRevert(ProjectDefinitions.InvalidTaskState.selector);
        vm.prank(agent);
        vault.withdraw(taskId, 1);

        vm.prank(agent);
        uint256 resume = vault.requestBudget(taskId, 10, PURPOSE);
        _approve(resume);

        assertEq(
            uint256(vault.getTask(taskId).status), uint256(ProjectDefinitions.TaskStatus.Stopped)
        );

        _fund(10);
        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertEq(
            uint256(vault.getTask(taskId).status), uint256(ProjectDefinitions.TaskStatus.Active)
        );
        assertTrue(vault.getTask(taskId).started);

        vm.prank(agent);
        vault.withdraw(taskId, 25);

        assertEq(vault.externalOutstanding(taskId), 30);
    }

    function testQueueTombstonesAdvanceOneAtATime() public {
        _fund(20);
        (uint256 firstTask, uint256 firstRequest) = _create(agent, 100);
        (uint256 secondTask, uint256 secondRequest) = _create(agent, 100);
        (uint256 thirdTask, uint256 thirdRequest) = _create(agent, 20);
        _approve(firstRequest);
        _approve(secondRequest);
        _approve(thirdRequest);

        vm.prank(owner);
        vault.cancel(firstTask);

        vm.prank(authority);
        vault.markOutOfBudget(secondTask);
        _apply(ProjectDefinitions.ApplyResult.Skipped);

        assertEq(vault.queueHead(), 1);
        assertEq(vault.getTask(thirdTask).available, 0);

        _apply(ProjectDefinitions.ApplyResult.Skipped);

        assertEq(vault.queueHead(), 2);

        _apply(ProjectDefinitions.ApplyResult.Funded);

        assertEq(vault.getTask(thirdTask).available, 20);
    }

    function testMaximumApprovalWaitsWithoutOverflowOrPartialAllocation() public {
        _fund(100);
        (uint256 taskId, uint256 requestId) = _create(agent, type(uint256).max);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Waiting);

        assertEq(vault.getTask(taskId).issued, 0);
        assertEq(vault.freeBalance(), 100);
    }

    function testRunningTaskSupplementCannotBypassAnEarlierApprovedTask() public {
        (uint256 runningTask,) = _funded(20);

        vm.prank(agent);
        vault.withdraw(runningTask, 5);

        (uint256 earlierTask, uint256 earlierRequest) = _create(other, 30);
        _approve(earlierRequest);

        vm.prank(agent);
        uint256 supplement = vault.requestBudget(runningTask, 10, PURPOSE);
        _approve(supplement);
        _fund(10);
        _apply(ProjectDefinitions.ApplyResult.Waiting);

        assertEq(vault.getTask(runningTask).available, 15);
        assertEq(vault.getTask(earlierTask).available, 0);
        assertEq(vault.getTask(runningTask).issued, 20);

        _fund(30);
        _apply(ProjectDefinitions.ApplyResult.Funded);
        assertEq(vault.getTask(earlierTask).available, 30);
        assertEq(vault.getTask(runningTask).available, 15);

        _apply(ProjectDefinitions.ApplyResult.Funded);
        assertEq(vault.getTask(runningTask).available, 25);
        assertEq(vault.getTask(runningTask).issued, 30);
        assertEq(vault.freeBalance(), 0);
    }
}
