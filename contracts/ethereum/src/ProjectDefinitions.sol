// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

/// @notice Shared Project value types, failures, and observable events.
/// @dev This declaration base owns no storage and implements no behavior.
abstract contract ProjectDefinitions {
    enum TaskStatus {
        None,
        Active,
        Stopped,
        OutOfBudget,
        Completed,
        Canceled
    }

    enum RequestStatus {
        None,
        Pending,
        Queued,
        Funded,
        Rejected,
        Removed
    }

    enum RequestKind {
        Initial,
        Supplement,
        Resume,
        Restoration
    }

    enum ApplyResult {
        Empty,
        Skipped,
        Waiting,
        Funded
    }

    enum InvoiceStatus {
        None,
        Open,
        Closed
    }

    enum RemovalReason {
        Canceled,
        OutOfBudget,
        Completed
    }

    enum StopReason {
        SupplementRejected,
        AuthorityOutOfBudget
    }

    struct Task {
        address agent;
        TaskStatus status;
        bool paused;
        bool started;
        uint256 available;
        uint256 issued;
        uint256 recovered;
        uint256 liveRequestId;
        bytes32 reportRef;
    }

    struct BudgetRequest {
        uint256 taskId;
        uint256 amount;
        bytes32 purposeRef;
        RequestStatus status;
        RequestKind kind;
    }

    struct Invoice {
        uint256 requested;
        bytes32 requestRef;
        InvoiceStatus status;
        uint256 intendedContribution;
        bytes32 responseRef;
        uint256 funded;
    }

    error Unauthorized();
    error InvalidAddress();
    error InvalidAsset();
    error InvalidAmount();
    error MissingPurpose();
    error UnknownTask();
    error UnknownRequest();
    error UnknownInvoice();
    error InvalidTaskState();
    error InvalidRequestState();
    error InvalidInvoiceState();
    error LiveRequestExists();
    error TaskIsPaused();
    error TaskIsNotPaused();
    error TaskAlreadyStarted();
    error TaskNotFunded();
    error ControlledRemainder();
    error InsufficientFunds();
    error ExcessReturn();
    error NonExactTransfer();

    event AuthorityChanged(address indexed previousAuthority, address indexed newAuthority);
    event ProjectFunded(uint256 amount, uint256 indexed invoiceId);
    event OwnerWithdrawal(uint256 amount);
    event TaskCreated(uint256 indexed taskId, address indexed agent, uint256 requestId);
    event BudgetRequested(
        uint256 indexed requestId,
        uint256 indexed taskId,
        uint256 amount,
        bytes32 purposeRef,
        RequestKind kind
    );
    event RequestApproved(uint256 indexed requestId, uint256 approvalSequence);
    event RequestRejected(uint256 indexed requestId, uint256 indexed taskId);
    event RequestRemoved(uint256 indexed requestId, RemovalReason reason);
    event QueueAdvanced(uint256 indexed requestId);
    event FundingWaiting(uint256 indexed requestId, uint256 required, uint256 free);
    event BudgetAllocated(uint256 indexed requestId, uint256 indexed taskId, uint256 amount);
    event AgentWithdrawal(uint256 indexed taskId, uint256 amount);
    event ExternalReturn(uint256 indexed taskId, uint256 amount);
    event ControlledRelease(uint256 indexed taskId, uint256 amount);
    event TaskPaused(uint256 indexed taskId);
    event BudgetReclaimed(uint256 indexed taskId, uint256 amount);
    event TaskContinued(uint256 indexed taskId);
    event TaskOutOfBudget(uint256 indexed taskId);
    event TaskStopped(uint256 indexed taskId, StopReason reason);
    event TaskCanceled(uint256 indexed taskId);
    event TaskFinalized(uint256 indexed taskId, uint256 expense, bytes32 reportRef);
    event ReplenishmentRequested(uint256 indexed invoiceId, uint256 amount, bytes32 noteRef);
    event ReplenishmentResponded(
        uint256 indexed invoiceId, uint256 intendedContribution, bytes32 noteRef
    );
}
