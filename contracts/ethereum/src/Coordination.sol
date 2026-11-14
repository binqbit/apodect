// SPDX-License-Identifier: MIT
pragma solidity 0.8.33;

/// @notice The fixed cross-chain wire and local custody records used by coordinated Projects.
library Coordination {
    enum Action {
        None,
        Reserve,
        ExpectDelivery,
        ExpectRefund,
        Dispatch,
        Allocate,
        ClosePlan,
        Reject,
        OutOfBudget,
        Pause,
        Reclaim,
        Continue,
        Cancel,
        RestoreRequest,
        OwnerWithdraw,
        Invalidate
    }

    enum PlanStatus {
        None,
        Open,
        Allocated,
        Closed
    }

    enum OutcomeCode {
        None,
        Executed,
        Failed,
        Invalidated,
        Pending
    }

    /// @dev ABI encoding is exactly twelve words. Narrow integers also reject noncanonical padding.
    struct Command {
        bytes32 domain;
        uint64 nonce;
        uint256 action;
        bytes32 operation;
        bytes32 related;
        uint256 task;
        uint256 request;
        uint64 amount;
        uint64 limit;
        bytes32 recipient;
        uint64 expires;
        bytes32 ref;
    }

    struct Plan {
        PlanStatus status;
        uint64 held;
    }

    struct Route {
        bytes32 plan;
        address escrow;
        uint64 expected;
        uint64 received;
        uint64 dispatched;
    }

    struct Outcome {
        bytes32 digest;
        uint256 action;
        OutcomeCode code;
        uint256 value;
    }
}
