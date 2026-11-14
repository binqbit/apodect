// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

import { Test } from "forge-std/Test.sol";
import { ProjectVault } from "../../src/ProjectVault.sol";
import { ProjectDefinitions } from "../../src/ProjectDefinitions.sol";
import { WorkspaceRegistry } from "../../src/WorkspaceRegistry.sol";
import { TestUSDC } from "../TestAssets.sol";

abstract contract ProjectVaultTestBase is Test {
    bytes32 internal constant PURPOSE = keccak256("task purpose");

    address internal owner;
    address internal authority;
    address internal agent;
    address internal other;

    TestUSDC internal token;
    WorkspaceRegistry internal registry;
    ProjectVault internal vault;
    uint256 internal workspaceId;

    function setUp() public {
        owner = makeAddr("owner");
        authority = makeAddr("authority");
        agent = makeAddr("agent");
        other = makeAddr("other");
        token = new TestUSDC();
        registry = new WorkspaceRegistry(address(token));

        vm.startPrank(owner);
        workspaceId = registry.createWorkspace();
        (, address project) = registry.createProject(workspaceId, authority);
        vault = ProjectVault(project);
        token.approve(project, type(uint256).max);
        vm.stopPrank();

        token.mint(owner, 1e24);

        vm.prank(agent);
        token.approve(address(vault), type(uint256).max);
    }

    function _fund(uint256 amount) internal {
        vm.prank(owner);
        vault.fund(amount, 0);
    }

    function _create(address taskAgent, uint256 amount) internal returns (uint256, uint256) {
        vm.prank(taskAgent);
        return vault.createTask(amount, PURPOSE);
    }

    function _approve(uint256 requestId) internal {
        vm.prank(authority);
        vault.approveRequest(requestId);
    }

    function _apply(ProjectDefinitions.ApplyResult expected) internal {
        vm.prank(authority);
        (, ProjectDefinitions.ApplyResult result) = vault.applyNext();

        assertEq(uint256(result), uint256(expected));
    }

    function _funded(uint256 amount) internal returns (uint256 taskId, uint256 requestId) {
        _fund(amount);
        (taskId, requestId) = _create(agent, amount);
        _approve(requestId);
        _apply(ProjectDefinitions.ApplyResult.Funded);
    }
}
