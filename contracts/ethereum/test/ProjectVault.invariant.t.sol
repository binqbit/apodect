// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { Test } from "forge-std/Test.sol";
import { StdInvariant } from "forge-std/StdInvariant.sol";
import { ProjectVault } from "../src/ProjectVault.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";
import { TestUSDC } from "./TestAssets.sol";

import { LifecycleHandler } from "./support/LifecycleHandler.sol";

contract ProjectVaultInvariantTest is StdInvariant, Test {
    ProjectVault private vault;
    TestUSDC private token;
    LifecycleHandler private handler;

    function setUp() public {
        token = new TestUSDC();
        vault = new ProjectVault(address(token), address(0xA11CE), address(0xA070));
        handler = new LifecycleHandler(vault, token);

        bytes4[] memory selectors = new bytes4[](17);
        selectors[0] = handler.fund.selector;
        selectors[1] = handler.create.selector;
        selectors[2] = handler.request.selector;
        selectors[3] = handler.approve.selector;
        selectors[4] = handler.reject.selector;
        selectors[5] = handler.applyNext.selector;
        selectors[6] = handler.withdraw.selector;
        selectors[7] = handler.returnPrincipal.selector;
        selectors[8] = handler.releasePrincipal.selector;
        selectors[9] = handler.pause.selector;
        selectors[10] = handler.reclaim.selector;
        selectors[11] = handler.restore.selector;
        selectors[12] = handler.continueTask.selector;
        selectors[13] = handler.cancel.selector;
        selectors[14] = handler.outOfBudget.selector;
        selectors[15] = handler.finalize.selector;
        selectors[16] = handler.ownerWithdraw.selector;

        targetSelector(FuzzSelector({ addr: address(handler), selectors: selectors }));
        targetContract(address(handler));
    }

    function invariantCustodyAndExternalPrincipalConserveActualFunding() public view {
        uint256 available;
        uint256 externalPrincipal;
        for (uint256 taskId = 1; taskId <= vault.taskCount(); ++taskId) {
            ProjectDefinitions.Task memory task = vault.getTask(taskId);
            available += task.available;
            externalPrincipal += vault.externalOutstanding(taskId);

            assertGe(task.issued, task.recovered + task.available);
        }

        assertEq(available, vault.allocatedBalance());
        assertEq(vault.accountedBalance(), token.balanceOf(address(vault)));
        assertEq(externalPrincipal, token.balanceOf(address(0xA6E17)));
        assertEq(
            vault.freeBalance() + available + externalPrincipal + handler.totalOwnerWithdrawals(),
            handler.totalFunding()
        );
    }

    function invariantTerminalTasksHaveNoExecutableRightsOrLiveRequest() public view {
        for (uint256 taskId = 1; taskId <= vault.taskCount(); ++taskId) {
            ProjectDefinitions.Task memory task = vault.getTask(taskId);
            if (
                task.status == ProjectDefinitions.TaskStatus.Completed
                    || task.status == ProjectDefinitions.TaskStatus.Canceled
            ) {
                assertEq(task.available, 0);
                assertEq(task.liveRequestId, 0);
            }
            if (task.status == ProjectDefinitions.TaskStatus.OutOfBudget) {
                assertEq(task.liveRequestId, 0);
            }
            if (task.status == ProjectDefinitions.TaskStatus.Canceled) {
                assertFalse(task.started);
                assertEq(vault.externalOutstanding(taskId), 0);
            }
            if (task.status == ProjectDefinitions.TaskStatus.Completed) {
                assertEq(vault.taskExpense(taskId), vault.externalOutstanding(taskId));
            }
        }
    }

    function invariantQueueCursorNeverPassesLiveUnfundedWork() public view {
        for (uint256 index; index < vault.queueLength(); ++index) {
            uint256 requestId = vault.approvalQueue(index);
            ProjectDefinitions.BudgetRequest memory request = vault.getRequest(requestId);
            if (index < vault.queueHead()) {
                assertTrue(
                    request.status == ProjectDefinitions.RequestStatus.Funded
                        || request.status == ProjectDefinitions.RequestStatus.Removed
                );
            } else {
                assertTrue(
                    request.status == ProjectDefinitions.RequestStatus.Queued
                        || request.status == ProjectDefinitions.RequestStatus.Removed
                );
            }
            if (request.status == ProjectDefinitions.RequestStatus.Queued) {
                assertEq(vault.getTask(request.taskId).liveRequestId, requestId);
            }
        }
    }
}
