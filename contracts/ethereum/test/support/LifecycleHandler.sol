// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { Test } from "forge-std/Test.sol";
import { ProjectVault } from "../../src/ProjectVault.sol";
import { ProjectDefinitions } from "../../src/ProjectDefinitions.sol";
import { TestUSDC } from "../TestAssets.sol";

/// @dev Actor-valid stateful actions. Unexpected contract reverts fail the invariant run.
contract LifecycleHandler is Test {
    bytes32 private constant PURPOSE = keccak256("invariant purpose");
    address private constant OWNER = address(0xA11CE);
    address private constant AUTHORITY = address(0xA070);
    address private constant AGENT = address(0xA6E17);

    ProjectVault public immutable vault;
    TestUSDC public immutable token;

    uint256 public totalFunding;
    uint256 public totalOwnerWithdrawals;

    constructor(ProjectVault vault_, TestUSDC token_) {
        vault = vault_;
        token = token_;

        vm.prank(OWNER);
        token.approve(address(vault), type(uint256).max);

        vm.prank(AGENT);
        token.approve(address(vault), type(uint256).max);
    }

    function fund(uint96 input) external {
        uint256 amount = bound(uint256(input), 1, 1e9);
        token.mint(OWNER, amount);

        vm.prank(OWNER);
        vault.fund(amount, 0);
        totalFunding += amount;
    }

    function create(uint96 input) external {
        if (vault.taskCount() >= 20) return;

        vm.prank(AGENT);
        vault.createTask(bound(uint256(input), 1, 1e6), PURPOSE);
    }

    function request(uint256 seed, uint96 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (task.liveRequestId != 0) return;
        if (
            task.status != ProjectDefinitions.TaskStatus.Stopped
                && !(task.status == ProjectDefinitions.TaskStatus.Active
                    && (task.issued == 0 || task.started))
        ) return;

        vm.prank(AGENT);
        vault.requestBudget(taskId, bound(uint256(input), 1, 1e6), PURPOSE);
    }

    function approve(uint256 seed) external {
        uint256 requestId = _pending(seed);
        if (requestId == 0) return;

        vm.prank(AUTHORITY);
        vault.approveRequest(requestId);
    }

    function reject(uint256 seed) external {
        uint256 requestId = _pending(seed);
        if (requestId == 0) return;

        vm.prank(AUTHORITY);
        vault.rejectRequest(requestId);
    }

    function applyNext() external {
        vm.prank(AUTHORITY);
        vault.applyNext();
    }

    function withdraw(uint256 seed, uint256 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (
            task.status != ProjectDefinitions.TaskStatus.Active || task.paused
                || task.available == 0
        ) {
            return;
        }

        vm.prank(AGENT);
        vault.withdraw(taskId, bound(input, 1, task.available));
    }

    function returnPrincipal(uint256 seed, uint256 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0 || !_nonterminal(vault.getTask(taskId))) return;
        uint256 externalPrincipal = vault.externalOutstanding(taskId);
        if (externalPrincipal == 0) return;

        vm.prank(AGENT);
        vault.returnFunds(taskId, bound(input, 1, externalPrincipal));
    }

    function releasePrincipal(uint256 seed, uint256 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (!_nonterminal(task) || task.available == 0) return;

        vm.prank(AGENT);
        vault.release(taskId, bound(input, 1, task.available));
    }

    function pause(uint256 seed) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (!_nonterminal(task) || task.paused) return;

        vm.prank(OWNER);
        vault.pause(taskId);
    }

    function reclaim(uint256 seed, uint256 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (!_nonterminal(task) || !task.paused || task.available == 0) return;

        vm.prank(OWNER);
        vault.reclaim(taskId, bound(input, 1, task.available));
    }

    function restore(uint256 seed, uint96 input) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (
            task.status != ProjectDefinitions.TaskStatus.Active || !task.paused || task.issued == 0
                || task.liveRequestId != 0
        ) return;

        vm.prank(AUTHORITY);
        vault.restore(taskId, bound(uint256(input), 1, 1e6), PURPOSE);
    }

    function continueTask(uint256 seed) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (
            task.status != ProjectDefinitions.TaskStatus.Active || !task.paused
                || task.available == 0
        ) {
            return;
        }

        vm.prank(OWNER);
        vault.continueTask(taskId);
    }

    function cancel(uint256 seed) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (!_nonterminal(task) || task.started) return;

        vm.prank(OWNER);
        vault.cancel(taskId);
    }

    function outOfBudget(uint256 seed) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (
            task.status != ProjectDefinitions.TaskStatus.Active
                && task.status != ProjectDefinitions.TaskStatus.Stopped
        ) return;

        vm.prank(AUTHORITY);
        vault.markOutOfBudget(taskId);
    }

    function finalize(uint256 seed) external {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return;

        ProjectDefinitions.Task memory task = vault.getTask(taskId);
        if (!_nonterminal(task) || task.issued == 0 || task.available != 0) return;

        vm.prank(AGENT);
        vault.finalize(taskId, bytes32(0));
    }

    function ownerWithdraw(uint256 input) external {
        uint256 available = vault.freeBalance();
        if (available == 0) return;
        uint256 amount = bound(input, 1, available);

        vm.prank(OWNER);
        vault.withdrawFree(amount);
        totalOwnerWithdrawals += amount;
    }

    function _pick(uint256 seed) private view returns (uint256) {
        uint256 count = vault.taskCount();
        return count == 0 ? 0 : seed % count + 1;
    }

    function _pending(uint256 seed) private view returns (uint256) {
        uint256 taskId = _pick(seed);
        if (taskId == 0) return 0;
        uint256 requestId = vault.getTask(taskId).liveRequestId;
        if (requestId == 0) return 0;
        return vault.getRequest(requestId).status == ProjectDefinitions.RequestStatus.Pending
            ? requestId
            : 0;
    }

    function _nonterminal(ProjectDefinitions.Task memory task) private pure returns (bool) {
        return task.status != ProjectDefinitions.TaskStatus.Completed
            && task.status != ProjectDefinitions.TaskStatus.Canceled;
    }
}
