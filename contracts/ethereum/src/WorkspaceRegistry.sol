// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { IERC20Metadata } from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import { ProjectVault } from "./ProjectVault.sol";

/// @title Apodect Workspace registry and Project factory
/// @notice Each created Project has a distinct contract address and token balance.
/// @dev Project identity is the registry deployment, Workspace ID, and that Workspace's nonce.
///      The registry never holds Project funds or grants itself Project management powers.
contract WorkspaceRegistry {
    struct Workspace {
        address owner;
        uint256 projectCount;
    }

    error Unauthorized();
    error UnknownWorkspace();
    error InvalidAsset();

    event WorkspaceCreated(uint256 indexed workspaceId, address indexed owner);
    event ProjectCreated(
        uint256 indexed workspaceId,
        uint256 indexed projectNonce,
        address indexed project,
        address authority
    );

    address public immutable asset;
    uint256 public workspaceCount;

    mapping(uint256 workspaceId => Workspace) public workspaces;
    mapping(uint256 workspaceId => mapping(uint256 projectNonce => address project)) public
        projects;

    constructor(address asset_) {
        if (asset_.code.length == 0 || IERC20Metadata(asset_).decimals() != 6) {
            revert InvalidAsset();
        }

        asset = asset_;
    }

    function createWorkspace() external returns (uint256 workspaceId) {
        workspaceId = ++workspaceCount;
        workspaces[workspaceId].owner = msg.sender;

        emit WorkspaceCreated(workspaceId, msg.sender);
    }

    function createProject(uint256 workspaceId, address authority)
        external
        returns (uint256 projectNonce, address project)
    {
        Workspace storage workspace = workspaces[workspaceId];
        if (workspace.owner == address(0)) revert UnknownWorkspace();
        if (msg.sender != workspace.owner) revert Unauthorized();

        projectNonce = ++workspace.projectCount;
        bytes32 salt = keccak256(abi.encode(workspaceId, projectNonce));
        project = address(new ProjectVault{ salt: salt }(asset, workspace.owner, authority));
        projects[workspaceId][projectNonce] = project;

        emit ProjectCreated(workspaceId, projectNonce, project, authority);
    }
}
