// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { ProjectVaultTestBase } from "./support/ProjectVaultTestBase.sol";
import { ProjectVault } from "../src/ProjectVault.sol";
import { ProjectDefinitions } from "../src/ProjectDefinitions.sol";
import { WorkspaceRegistry } from "../src/WorkspaceRegistry.sol";
import { WrongDecimalsToken } from "./TestAssets.sol";

contract WorkspaceRolesTest is ProjectVaultTestBase {
    function testEveryRestrictedVaultOperationRejectsTheWrongActor() public {
        (uint256 taskId, uint256 requestId) = _funded(20);

        vm.prank(authority);
        uint256 invoiceId = vault.requestReplenishment(10, PURPOSE);

        // Owner actions must also reject authority; being appointed grants no ownership power.
        bytes[] memory ownerCalls = new bytes[](8);
        ownerCalls[0] = abi.encodeCall(ProjectVault.setAuthority, (other));
        ownerCalls[1] = abi.encodeCall(ProjectVault.fund, (1, 0));
        ownerCalls[2] = abi.encodeCall(ProjectVault.withdrawFree, (1));
        ownerCalls[3] = abi.encodeCall(ProjectVault.cancel, (taskId));
        ownerCalls[4] = abi.encodeCall(ProjectVault.pause, (taskId));
        ownerCalls[5] = abi.encodeCall(ProjectVault.reclaim, (taskId, 1));
        ownerCalls[6] = abi.encodeCall(ProjectVault.continueTask, (taskId));
        ownerCalls[7] = abi.encodeCall(ProjectVault.respondReplenishment, (invoiceId, 1, PURPOSE));

        for (uint256 i; i < ownerCalls.length; ++i) {
            vm.prank(authority);
            (bool succeeded, bytes memory result) = address(vault).call(ownerCalls[i]);
            assertFalse(succeeded);
            assertEq(result, abi.encodeWithSelector(ProjectDefinitions.Unauthorized.selector));
        }

        // Funding a Project does not allow its owner to make authority decisions.
        bytes[] memory authorityCalls = new bytes[](6);
        authorityCalls[0] = abi.encodeCall(ProjectVault.approveRequest, (requestId));
        authorityCalls[1] = abi.encodeCall(ProjectVault.rejectRequest, (requestId));
        authorityCalls[2] = abi.encodeCall(ProjectVault.applyNext, ());
        authorityCalls[3] = abi.encodeCall(ProjectVault.markOutOfBudget, (taskId));
        authorityCalls[4] = abi.encodeCall(ProjectVault.restore, (taskId, 1, PURPOSE));
        authorityCalls[5] = abi.encodeCall(ProjectVault.requestReplenishment, (1, PURPOSE));

        for (uint256 i; i < authorityCalls.length; ++i) {
            vm.prank(owner);
            (bool succeeded, bytes memory result) = address(vault).call(authorityCalls[i]);
            assertFalse(succeeded);
            assertEq(result, abi.encodeWithSelector(ProjectDefinitions.Unauthorized.selector));
        }

        // A second task agent cannot operate the first agent's task or claim its allocation.
        (uint256 otherTaskId,) = _create(other, 1);
        assertEq(vault.getTask(otherTaskId).agent, other);
        bytes[] memory agentCalls = new bytes[](5);
        agentCalls[0] = abi.encodeCall(ProjectVault.requestBudget, (taskId, 1, PURPOSE));
        agentCalls[1] = abi.encodeCall(ProjectVault.withdraw, (taskId, 1));
        agentCalls[2] = abi.encodeCall(ProjectVault.returnFunds, (taskId, 1));
        agentCalls[3] = abi.encodeCall(ProjectVault.release, (taskId, 1));
        agentCalls[4] = abi.encodeCall(ProjectVault.finalize, (taskId, PURPOSE));

        for (uint256 i; i < agentCalls.length; ++i) {
            vm.prank(other);
            (bool succeeded, bytes memory result) = address(vault).call(agentCalls[i]);
            assertFalse(succeeded);
            assertEq(result, abi.encodeWithSelector(ProjectDefinitions.Unauthorized.selector));
        }

        assertEq(vault.authority(), authority);
        assertEq(vault.getTask(taskId).available, 20);
        assertEq(vault.allocatedBalance(), 20);
    }

    function testWorkspaceNoncesAndProjectCustodyAreSeparate() public {
        _fund(100);

        vm.prank(owner);
        (uint256 nonce, address secondAddress) = registry.createProject(workspaceId, authority);
        ProjectVault second = ProjectVault(secondAddress);

        assertEq(nonce, 2);
        assertTrue(secondAddress != address(vault));
        assertEq(registry.projects(workspaceId, 1), address(vault));
        assertEq(registry.projects(workspaceId, 2), secondAddress);

        vm.prank(agent);
        (uint256 taskId, uint256 requestId) = second.createTask(1, PURPOSE);

        vm.prank(authority);
        second.approveRequest(requestId);

        vm.prank(authority);
        (, ProjectDefinitions.ApplyResult result) = second.applyNext();

        assertEq(uint256(result), uint256(ProjectDefinitions.ApplyResult.Waiting));
        assertEq(second.getTask(taskId).available, 0);
        assertEq(token.balanceOf(secondAddress), 0);
        assertEq(vault.freeBalance(), 100);

        vm.expectRevert(WorkspaceRegistry.Unauthorized.selector);
        vm.prank(other);
        registry.createProject(workspaceId, authority);

        vm.expectRevert(WorkspaceRegistry.UnknownWorkspace.selector);
        registry.createProject(999, authority);
    }

    function testAuthorityRotationPreservesApprovedTasksPendingRequestsAndInvoices() public {
        _fund(20);
        (uint256 taskId, uint256 requestId) = _create(agent, 10);
        (, uint256 pending) = _create(agent, 10);
        _approve(requestId);

        vm.prank(authority);
        uint256 invoiceId = vault.requestReplenishment(100, PURPOSE);

        vm.prank(owner);
        vault.setAuthority(other);

        vm.expectRevert(ProjectDefinitions.Unauthorized.selector);
        vm.prank(authority);
        vault.approveRequest(pending);

        vm.prank(other);
        vault.approveRequest(pending);

        vm.prank(other);
        vault.applyNext();

        assertEq(vault.getTask(taskId).available, 10);

        vm.prank(owner);
        vault.respondReplenishment(invoiceId, 60, PURPOSE);

        assertEq(
            uint256(vault.getInvoice(invoiceId).status),
            uint256(ProjectDefinitions.InvoiceStatus.Closed)
        );
    }

    function testConstructorsRejectWrongAssetAndMissingRoles() public {
        WrongDecimalsToken wrong = new WrongDecimalsToken();

        vm.expectRevert(ProjectDefinitions.InvalidAsset.selector);
        new ProjectVault(address(wrong), owner, authority);

        vm.expectRevert(WorkspaceRegistry.InvalidAsset.selector);
        new WorkspaceRegistry(address(wrong));

        vm.expectRevert(ProjectDefinitions.InvalidAsset.selector);
        new ProjectVault(address(0), owner, authority);

        vm.expectRevert(ProjectDefinitions.InvalidAddress.selector);
        new ProjectVault(address(token), address(0), authority);

        vm.expectRevert(ProjectDefinitions.InvalidAddress.selector);
        new ProjectVault(address(token), owner, address(0));
    }
}
