// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { Test, Vm } from "forge-std/Test.sol";
import { ProjectVault } from "../src/ProjectVault.sol";
import { ProjectDefinitions as D } from "../src/ProjectDefinitions.sol";
import { Coordination as C } from "../src/Coordination.sol";
import { RouteEscrow } from "../src/RouteEscrow.sol";
import { TestUSDC } from "./TestAssets.sol";

contract CoordinationTest is Test {
    uint256 private constant KEY = 1;
    bytes32 private constant DOMAIN =
        hex"d4ea16dd07a1c27658dc0cd4ad445a8f5f6716a586db50e695d3d170fc71ea63";
    bytes32 private constant PLAN = keccak256("plan");
    bytes32 private constant ROUTE = keccak256("route");
    bytes32 private constant PURPOSE = keccak256("purpose");
    address private owner = makeAddr("owner");
    address private authority = makeAddr("authority");
    address private agent = makeAddr("agent");
    address private provider = makeAddr("provider");
    TestUSDC private token;
    ProjectVault private vault;
    uint256 private nextOperation;

    function setUp() public {
        token = new TestUSDC();
        vault = new ProjectVault(address(token), owner, authority);
        vm.startPrank(owner);
        vault.bindCoordination(DOMAIN, _publicKey());
        token.approve(address(vault), type(uint256).max);
        vm.stopPrank();
        token.mint(owner, 1000e6);
        token.mint(provider, 1000e6);
        vm.prank(agent);
        token.approve(address(vault), type(uint256).max);
    }

    function _publicKey() private pure returns (bytes memory) {
        return hex"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";
    }

    function _command(
        C.Action action,
        bytes32 plan,
        uint256 task,
        uint256 request,
        uint64 amount,
        bytes32 ref
    ) private returns (C.Command memory c) {
        c.domain = DOMAIN;
        c.action = uint256(action);
        c.operation = bytes32(++nextOperation);
        c.related = plan;
        c.task = task;
        c.request = request;
        c.amount = amount;
        c.expires = uint64(block.timestamp + 1000);
        c.ref = ref;
    }

    function _execute(C.Command memory c) private returns (uint256) {
        bytes memory wire = abi.encode(c);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(KEY, sha256(wire));
        return vault.executeCommand(wire, abi.encodePacked(r, s), v - 27);
    }

    function _fails(C.Command memory c, bytes4 expected) private {
        bytes memory wire = abi.encode(c);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(KEY, sha256(wire));
        if (expected == bytes4(0)) vm.expectRevert();
        else vm.expectRevert(expected);
        vault.executeCommand(wire, abi.encodePacked(r, s), v - 27);
    }

    function _fund(uint256 amount) private {
        vm.prank(owner);
        vault.fund(amount, 0);
    }

    function _reserve(bytes32 plan, uint64 amount) private {
        _execute(_command(C.Action.Reserve, plan, 0, 0, amount, 0));
    }

    function _create(uint64 amount) private returns (uint256 task, uint256 request) {
        vm.prank(agent);
        return vault.createTask(amount, PURPOSE);
    }

    function _allocated(uint64 amount) private returns (uint256 task, uint256 request) {
        _fund(amount);
        (task, request) = _create(amount);
        _reserve(PLAN, amount);
        _execute(_command(C.Action.Allocate, PLAN, task, request, amount, 0));
    }

    function _deposit(bool refund, uint64 amount) private {
        address escrow = vault.getRoute(ROUTE, refund).escrow;
        vm.prank(provider);
        token.transfer(escrow, amount);
    }

    function testBindingIsOwnerAuthorizedFreshAndPermanent() public {
        assertEq(vault.coordinatorSigner(), vm.addr(KEY));
        ProjectVault fresh = new ProjectVault(address(token), owner, authority);
        vm.expectRevert(D.Unauthorized.selector);
        fresh.bindCoordination(DOMAIN, _publicKey());
        vm.prank(agent);
        fresh.createTask(1, PURPOSE);
        vm.prank(owner);
        vm.expectRevert(ProjectVault.InvalidCoordination.selector);
        fresh.bindCoordination(DOMAIN, _publicKey());
        vm.prank(owner);
        vm.expectRevert(ProjectVault.CoordinatedProject.selector);
        vault.bindCoordination(DOMAIN, _publicKey());
    }

    function testFuzzPlanIsolationAndClosureConserveActualCash(uint64 first, uint64 second) public {
        first = uint64(bound(first, 1, 1000e6 / 2));
        second = uint64(bound(second, 1, 1000e6 / 2));
        _fund(uint256(first) + second);
        _reserve(PLAN, first);
        bytes32 another = keccak256("another");
        _reserve(another, second);
        assertEq(vault.freeBalance(), 0);
        _execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0));
        assertEq(vault.freeBalance(), first);
        assertEq(vault.getPlan(another).held, second);
        assertEq(vault.accountedBalance(), token.balanceOf(address(vault)));
        _fails(_command(C.Action.Reserve, PLAN, 0, 0, 0, 0), ProjectVault.InvalidPlan.selector);
    }

    function testAuthenticationRejectsWrongKeyDomainPaddingAndHighS() public {
        C.Command memory c = _command(C.Action.Reserve, PLAN, 0, 0, 0, 0);
        bytes memory wire = abi.encode(c);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(2, sha256(wire));
        vm.expectRevert(D.Unauthorized.selector);
        vault.executeCommand(wire, abi.encodePacked(r, s), v - 27);
        (v, r, s) = vm.sign(KEY, sha256(wire));
        uint256 order = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141;
        vm.expectRevert(D.Unauthorized.selector);
        vault.executeCommand(wire, abi.encodePacked(r, bytes32(order - uint256(s))), (v - 27) ^ 1);
        c.domain = bytes32(uint256(9));
        _fails(c, ProjectVault.InvalidCommand.selector);
        // Narrow nonce decoding rejects high nonzero padding, even with a valid signature.
        wire = abi.encode(_command(C.Action.Reserve, PLAN, 0, 0, 0, 0));
        assembly ("memory-safe") { mstore(add(wire, 64), shl(128, 1)) }
        (v, r, s) = vm.sign(KEY, sha256(wire));
        vm.expectRevert();
        vault.executeCommand(wire, abi.encodePacked(r, s), v - 27);
    }

    function testDirectManagementBypassesAreDisabled() public {
        (uint256 task, uint256 request) = _create(1);
        bytes[] memory calls = new bytes[](14);
        calls[0] = abi.encodeCall(vault.setAuthority, (authority));
        calls[1] = abi.encodeCall(vault.withdrawFree, (1));
        calls[2] = abi.encodeCall(vault.approveRequest, (request));
        calls[3] = abi.encodeCall(vault.rejectRequest, (request));
        calls[4] = abi.encodeCall(vault.applyNext, ());
        calls[5] = abi.encodeCall(vault.cancel, (task));
        calls[6] = abi.encodeCall(vault.markOutOfBudget, (task));
        calls[7] = abi.encodeCall(vault.pause, (task));
        calls[8] = abi.encodeCall(vault.reclaim, (task, 1));
        calls[9] = abi.encodeCall(vault.restore, (task, 1, PURPOSE));
        calls[10] = abi.encodeCall(vault.continueTask, (task));
        calls[11] = abi.encodeCall(vault.requestReplenishment, (1, PURPOSE));
        calls[12] = abi.encodeCall(vault.respondReplenishment, (1, 1, PURPOSE));
        calls[13] = abi.encodeCall(vault.fund, (1, 1));
        for (uint256 i; i < calls.length; ++i) {
            bool authorityCall = i == 2 || i == 3 || i == 4 || i == 6 || i == 9 || i == 11;
            vm.prank(authorityCall ? authority : owner);
            (bool ok, bytes memory reason) = address(vault).call(calls[i]);
            assertFalse(ok);
            assertEq(bytes4(reason), ProjectVault.CoordinatedProject.selector);
        }
    }

    function testFundingReceiptsRequireActualOwnerTokensAndRemainUnique() public {
        vm.prank(agent);
        vm.expectRevert(D.Unauthorized.selector);
        vault.coordinatedFund(50, PLAN);
        token.setFailTransfers(true);
        vm.prank(owner);
        vm.expectRevert();
        vault.coordinatedFund(50, PLAN);
        assertEq(vault.fundingSequence(), 0);
        assertEq(vault.freeBalance(), 0);
        token.setFailTransfers(false);
        vm.recordLogs();
        vm.prank(owner);
        vault.coordinatedFund(50, PLAN);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        Vm.Log memory receipt = logs[logs.length - 1];
        assertEq(receipt.emitter, address(vault));
        assertEq(receipt.topics[0], keccak256("FundingRecorded(bytes32,bytes32,bytes)"));
        assertEq(receipt.topics[1], bytes32(uint256(1)));
        assertEq(receipt.topics[2], DOMAIN);
        bytes memory payload = abi.decode(receipt.data, (bytes));
        assertEq(payload.length, 160);
        bytes32[5] memory words = abi.decode(payload, (bytes32[5]));
        assertEq(words[0], DOMAIN);
        assertEq(words[1], bytes32(uint256(1)));
        assertEq(words[2], PLAN);
        assertEq(uint256(words[3]), 50);
        assertEq(words[4], bytes32(uint256(uint160(owner))));
        _fund(20);
        assertEq(vault.fundingSequence(), 2);
        assertEq(vault.freeBalance(), 70);
        assertEq(token.balanceOf(address(vault)), 70);
    }

    function testOutOfBudgetAndSignedOwnerWithdrawalCannotSpendHeldFunds() public {
        _fund(100);
        (uint256 task, uint256 request) = _create(60);
        _reserve(PLAN, 60);
        _execute(_command(C.Action.OutOfBudget, 0, task, 0, 0, 0));
        _fails(
            _command(C.Action.Allocate, PLAN, task, request, 60, 0), D.InvalidRequestState.selector
        );
        C.Command memory withdrawal = _command(C.Action.OwnerWithdraw, 0, 0, 0, 41, 0);
        withdrawal.recipient = bytes32(uint256(uint160(owner)));
        _fails(withdrawal, D.InsufficientFunds.selector);
        withdrawal.amount = 40;
        withdrawal.recipient = bytes32(uint256(uint160(provider)));
        _fails(withdrawal, ProjectVault.InvalidCommand.selector);
        withdrawal.recipient = bytes32(uint256(uint160(owner)));
        _execute(withdrawal);
        assertEq(vault.freeBalance(), 0);
        assertEq(vault.heldBalance(), 60);
        _execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0));
        assertEq(vault.freeBalance(), 60);
    }

    function testIndependentHoldsReplayConflictAndExpiry() public {
        _fund(100);
        C.Command memory c = _command(C.Action.Reserve, PLAN, 0, 0, 60, 0);
        assertEq(_execute(c), 60);
        assertEq(_execute(c), 60);
        assertEq(vault.freeBalance(), 40);
        _reserve(keccak256("second"), 40);
        assertEq(vault.heldBalance(), 100);
        c.amount = 61;
        _fails(c, ProjectVault.CommandConflict.selector);
        c.amount = 60;
        vm.warp(c.expires + 1);
        assertEq(_execute(c), 60);
        c.operation = bytes32(++nextOperation);
        _fails(c, ProjectVault.CommandExpired.selector);
    }

    function testFullDeliveryWaitsForPartialAndNeverCreditsDonationsTwice() public {
        (uint256 task, uint256 request) = _create(60);
        _reserve(PLAN, 0);
        address predicted = vault.predictRouteAddress(ROUTE, false);
        assertNotEq(predicted, vault.predictRouteAddress(ROUTE, true));
        assertEq(vault.getRoute(ROUTE, false).escrow, address(0));
        _execute(_command(C.Action.ExpectDelivery, PLAN, 0, 0, 60, ROUTE));
        assertEq(vault.getRoute(ROUTE, false).escrow, predicted);
        assertEq(vault.predictRouteAddress(ROUTE, false), predicted);
        vm.prank(provider);
        token.transfer(address(vault), 999);
        assertEq(vault.accountedBalance(), 0);
        _deposit(false, 35);
        assertEq(vault.collectRoute(ROUTE, false), 0);
        assertEq(vault.heldBalance(), 0);
        C.Command memory allocation = _command(C.Action.Allocate, PLAN, task, request, 60, 0);
        _fails(allocation, D.InsufficientFunds.selector);
        _deposit(false, 30);
        assertEq(vault.collectRoute(ROUTE, false), 60);
        assertEq(vault.collectRoute(ROUTE, false), 0);
        assertEq(token.balanceOf(vault.getRoute(ROUTE, false).escrow), 5);
        _execute(allocation);
        vm.prank(agent);
        vault.withdraw(task, 40);
        vm.prank(agent);
        vault.returnFunds(task, 15);
        vm.prank(agent);
        vault.release(task, 20);
        vm.prank(agent);
        vault.finalize(task, PURPOSE);
        assertEq(vault.taskExpense(task), 25);
        assertEq(vault.freeBalance(), 35);
    }

    function testLateDeliveryAfterCloseReturnsActualCashWithoutRights() public {
        _reserve(PLAN, 0);
        _execute(_command(C.Action.ExpectDelivery, PLAN, 0, 0, 60, ROUTE));
        _execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0));
        _deposit(false, 60);
        assertEq(vault.collectRoute(ROUTE, false), 60);
        assertEq(vault.freeBalance(), 60);
        assertEq(vault.heldBalance(), 0);
        assertEq(uint256(vault.getPlan(PLAN).status), uint256(C.PlanStatus.Closed));
    }

    function testDispatchUnknownStaysSpentAndPartialRefundIsBounded() public {
        _fund(100);
        _reserve(PLAN, 61);
        _execute(_command(C.Action.ExpectRefund, PLAN, 0, 0, 61, ROUTE));
        C.Command memory send = _command(C.Action.Dispatch, PLAN, 0, 0, 61, ROUTE);
        send.recipient = bytes32(uint256(uint160(provider)));
        uint256 before = token.balanceOf(provider);
        _execute(send);
        // A worker losing the response can resend the identical command without another transfer.
        _execute(send);
        assertEq(token.balanceOf(provider), before + 61);
        vm.warp(block.timestamp + 5000);
        _execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0));
        assertEq(vault.freeBalance(), 39);
        _deposit(true, 20);
        assertEq(vault.collectRoute(ROUTE, true), 20);
        assertEq(vault.collectRoute(ROUTE, true), 0);
        _deposit(true, 45);
        assertEq(vault.collectRoute(ROUTE, true), 41);
        assertEq(vault.freeBalance(), 100);
        assertEq(token.balanceOf(vault.getRoute(ROUTE, true).escrow), 4);
        assertEq(vault.getRoute(ROUTE, true).dispatched, 61);
    }

    function testTransferFailureIsNotTerminalAndInvalidationClosesIt() public {
        _fund(61);
        _reserve(PLAN, 61);
        _execute(_command(C.Action.ExpectRefund, PLAN, 0, 0, 61, ROUTE));
        C.Command memory send = _command(C.Action.Dispatch, PLAN, 0, 0, 61, ROUTE);
        send.recipient = bytes32(uint256(uint160(provider)));
        token.setFailTransfers(true);
        _fails(send, bytes4(0));
        assertEq(vault.getPlan(PLAN).held, 61);
        assertEq(uint256(vault.getCommandOutcome(send.operation).code), 0);
        C.Command memory invalidate = _command(C.Action.Invalidate, 0, 0, 0, 0, send.operation);
        assertEq(_execute(invalidate), uint256(C.OutcomeCode.Invalidated));
        token.setFailTransfers(false);
        _fails(send, ProjectVault.CommandInvalidated.selector);
        assertEq(_execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0)), 61);
    }

    function testAlreadyAllocatedAndExecutedCannotBeRolledBackByAbort() public {
        (uint256 task,) = _allocated(60);
        assertEq(_execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0)), 0);
        assertEq(vault.getTask(task).available, 60);
        assertEq(uint256(vault.getPlan(PLAN).status), uint256(C.PlanStatus.Allocated));
        assertEq(
            _execute(_command(C.Action.Invalidate, 0, 0, 0, 0, bytes32(uint256(2)))),
            uint256(C.OutcomeCode.Executed)
        );
        assertEq(vault.freeBalance(), 0);
    }

    function testCancelWinsAllocationRaceAndControlNoncePreventsReordering() public {
        _fund(60);
        (uint256 task, uint256 request) = _create(60);
        _reserve(PLAN, 60);
        C.Command memory allocate = _command(C.Action.Allocate, PLAN, task, request, 60, 0);
        C.Command memory cancel = _command(C.Action.Cancel, 0, task, 0, 0, 0);
        _execute(cancel);
        _fails(allocate, D.InvalidRequestState.selector);
        _execute(_command(C.Action.ClosePlan, PLAN, 0, 0, 0, 0));
        assertEq(vault.freeBalance(), 60);
        assertEq(vault.controlNonce(task), 1);
        assertEq(uint256(vault.getTask(task).status), uint256(D.TaskStatus.Canceled));
    }

    function testPauseReclaimRestoreContinueAndRejectedSupplementResume() public {
        (uint256 task,) = _allocated(60);
        _execute(_command(C.Action.Pause, 0, task, 0, 0, 0));
        C.Command memory reclaim = _command(C.Action.Reclaim, 0, task, 0, 20, 0);
        _fails(reclaim, ProjectVault.StaleControl.selector);
        reclaim.nonce = 1;
        _execute(reclaim);
        uint256 restored = _execute(_command(C.Action.RestoreRequest, 0, task, 0, 20, PURPOSE));
        assertEq(uint256(vault.getRequest(restored).status), uint256(D.RequestStatus.Pending));
        bytes32 second = keccak256("restoration");
        _reserve(second, 20);
        _execute(_command(C.Action.Allocate, second, task, restored, 20, 0));
        assertTrue(vault.getTask(task).paused);
        C.Command memory resume = _command(C.Action.Continue, 0, task, 0, 0, 0);
        resume.nonce = 2;
        _execute(resume);
        vm.prank(agent);
        vault.withdraw(task, 10);
        vm.prank(agent);
        uint256 supplement = vault.requestBudget(task, 15, PURPOSE);
        _execute(_command(C.Action.Reject, 0, task, supplement, 0, 0));
        assertEq(uint256(vault.getTask(task).status), uint256(D.TaskStatus.Stopped));
        vm.prank(agent);
        uint256 fresh = vault.requestBudget(task, 15, PURPOSE);
        _fund(15);
        bytes32 third = keccak256("resume");
        _reserve(third, 15);
        _execute(_command(C.Action.Allocate, third, task, fresh, 15, 0));
        assertEq(uint256(vault.getTask(task).status), uint256(D.TaskStatus.Active));
    }

    function testFinalizationWinsPendingSupplementAndNeverResurrectsTask() public {
        (uint256 task,) = _allocated(60);
        vm.prank(agent);
        vault.withdraw(task, 60);
        vm.prank(agent);
        uint256 supplement = vault.requestBudget(task, 5, PURPOSE);
        _fund(5);
        bytes32 second = keccak256("supplement");
        _reserve(second, 5);
        vm.prank(agent);
        vault.finalize(task, PURPOSE);
        _fails(
            _command(C.Action.Allocate, second, task, supplement, 5, 0),
            D.InvalidRequestState.selector
        );
        _execute(_command(C.Action.ClosePlan, second, 0, 0, 0, 0));
        assertEq(vault.freeBalance(), 5);
    }

    function testRecycledCashCannotOverflowCoordinatedCumulativeIssuance() public {
        uint64 maximum = type(uint64).max;
        token.mint(owner, maximum);
        (uint256 task,) = _allocated(maximum);
        vm.startPrank(agent);
        vault.withdraw(task, maximum);
        vault.returnFunds(task, maximum);
        uint256 request = vault.requestBudget(task, 1, PURPOSE);
        vm.stopPrank();

        bytes32 second = keccak256("recycled-cash");
        _reserve(second, 1);
        C.Command memory allocation = _command(C.Action.Allocate, second, task, request, 1, 0);
        _fails(allocation, ProjectVault.AmountOverflow.selector);

        assertEq(vault.getTask(task).issued, maximum);
        assertEq(vault.getTask(task).recovered, maximum);
        assertEq(vault.getTask(task).available, 0);
        assertEq(vault.getPlan(second).held, 1);
        assertEq(uint256(vault.getRequest(request).status), uint256(D.RequestStatus.Pending));
        assertEq(uint256(vault.getCommandOutcome(allocation.operation).code), 0);
        bytes32[39] memory words = abi.decode(
            vault.checkpoint(keccak256("still-representable"), task, request, second, 0, false, 0),
            (bytes32[39])
        );
        assertEq(uint256(words[12]), maximum);
        assertEq(uint256(words[13]), maximum);
    }

    function testCheckpointIsExactImmutableWireWithActualBlockIdentity() public {
        (uint256 task, uint256 request) = _allocated(60);
        bytes32 observation = keccak256("observation");
        vm.recordLogs();
        bytes memory payload =
            vault.checkpoint(observation, task, request, PLAN, ROUTE, false, bytes32(uint256(2)));
        assertEq(payload.length, 1248);
        bytes32[39] memory words = abi.decode(payload, (bytes32[39]));
        assertEq(words[0], DOMAIN);
        assertEq(words[1], observation);
        assertEq(uint256(words[2]), 1);
        assertEq(uint256(words[3]), block.number);
        assertEq(uint256(words[4]), block.timestamp);
        assertEq(uint256(words[8]), 0);
        assertEq(uint256(words[11]), 60);
        assertEq(uint256(words[21]), 2);
        assertEq(uint256(words[24]), 2);
        assertEq(uint256(words[35]), 1);
        assertEq(uint256(words[36]), 60);
        assertEq(uint256(words[38]), 0);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertEq(logs[0].emitter, address(vault));
        assertEq(logs[0].topics[0], keccak256("CheckpointRecorded(bytes32,bytes32,bytes)"));
        assertEq(abi.decode(logs[0].data, (bytes)), payload);
        vm.expectRevert(ProjectVault.InvalidCommand.selector);
        vault.checkpoint(observation, 0, 0, 0, 0, false, 0);
        payload = vault.checkpoint(keccak256("next"), 999, 999, 0, 0, false, 0);
        words = abi.decode(payload, (bytes32[39]));
        assertEq(uint256(words[2]), 2);
        assertEq(uint256(words[8]), 255);
        assertEq(uint256(words[21]), 255);
        assertEq(uint256(words[7]), 999);
    }
}
