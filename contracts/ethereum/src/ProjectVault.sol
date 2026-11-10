// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { IERC20Metadata } from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { ProjectDefinitions } from "./ProjectDefinitions.sol";
import { ExactToken } from "./ExactToken.sol";

/// @title Apodect locally funded Project
/// @notice Holds one Project's token principal and enforces its local task lifecycle.
/// @dev The owner, asset, and task agents are immutable. This contract has no cross-chain authority,
///      delegated token approvals, upgrade entry point, or arbitrary external-call facility.
contract ProjectVault is ProjectDefinitions, ReentrancyGuard {
    using ExactToken for IERC20;

    IERC20 public immutable asset;
    address public immutable owner;
    address public authority;

    /// @notice Attributed token principal available for allocation or owner withdrawal.
    uint256 public freeBalance;

    /// @notice Aggregate principal still controlled by live task allocations.
    uint256 public allocatedBalance;

    uint256 public taskCount;
    uint256 public requestCount;
    uint256 public invoiceCount;
    uint256 public queueHead;

    mapping(uint256 taskId => Task) private _tasks;
    mapping(uint256 requestId => BudgetRequest) private _requests;
    mapping(uint256 invoiceId => Invoice) private _invoices;

    uint256[] public approvalQueue;

    constructor(address asset_, address owner_, address authority_) {
        if (owner_ == address(0) || authority_ == address(0)) revert InvalidAddress();
        if (asset_.code.length == 0 || IERC20Metadata(asset_).decimals() != 6) {
            revert InvalidAsset();
        }

        asset = IERC20(asset_);
        owner = owner_;
        authority = authority_;
    }

    modifier onlyOwner() {
        if (msg.sender != owner) revert Unauthorized();
        _;
    }

    modifier onlyAuthority() {
        if (msg.sender != authority) revert Unauthorized();
        _;
    }

    /// @notice Changes future authority actions without invalidating existing work or invoices.
    function setAuthority(address newAuthority) external onlyOwner nonReentrant {
        if (newAuthority == address(0)) revert InvalidAddress();

        address previous = authority;
        authority = newAuthority;

        emit AuthorityChanged(previous, newAuthority);
    }

    /// @notice Funds this Project from its owner. An optional invoice must already be closed.
    /// @dev Only the actual exact token transfer credits cash; direct donations remain unattributed.
    function fund(uint256 amount, uint256 invoiceId) external onlyOwner nonReentrant {
        _positive(amount);
        if (invoiceId != 0) {
            Invoice storage invoice = _invoice(invoiceId);
            if (invoice.status != InvoiceStatus.Closed) revert InvalidInvoiceState();
            invoice.funded += amount;
        }

        freeBalance += amount;
        asset.receiveExact(owner, amount);

        emit ProjectFunded(amount, invoiceId);
    }

    function withdrawFree(uint256 amount) external onlyOwner nonReentrant {
        _positive(amount);
        if (amount > freeBalance) revert InsufficientFunds();

        freeBalance -= amount;
        asset.sendExact(owner, amount);

        emit OwnerWithdrawal(amount);
    }

    /// @notice Any signer may request funds for a task whose agent is that signer.
    function createTask(uint256 amount, bytes32 purposeRef)
        external
        nonReentrant
        returns (uint256 taskId, uint256 requestId)
    {
        _requestInputs(amount, purposeRef);

        taskId = ++taskCount;
        Task storage task = _tasks[taskId];
        task.agent = msg.sender;
        task.status = TaskStatus.Active;
        requestId = _newRequest(taskId, task, amount, purposeRef, RequestKind.Initial);

        emit TaskCreated(taskId, msg.sender, requestId);
    }

    /// @notice Replaces a rejected initial request, requests a running task supplement, or resumes
    ///         a stopped task through a new approval. An owner pause stays independent.
    function requestBudget(uint256 taskId, uint256 amount, bytes32 purposeRef)
        external
        nonReentrant
        returns (uint256 requestId)
    {
        Task storage task = _agentTask(taskId);
        _requestInputs(amount, purposeRef);

        RequestKind kind;
        if (task.status == TaskStatus.Stopped) {
            kind = RequestKind.Resume;
        } else if (task.status == TaskStatus.Active && task.issued == 0) {
            kind = RequestKind.Initial;
        } else if (task.status == TaskStatus.Active && task.started) {
            kind = RequestKind.Supplement;
        } else {
            revert InvalidTaskState();
        }

        requestId = _newRequest(taskId, task, amount, purposeRef, kind);
    }

    function approveRequest(uint256 requestId) external onlyAuthority nonReentrant {
        BudgetRequest storage request = _request(requestId);
        if (request.status != RequestStatus.Pending) revert InvalidRequestState();

        _approve(requestId, request);
    }

    function rejectRequest(uint256 requestId) external onlyAuthority nonReentrant {
        BudgetRequest storage request = _request(requestId);
        if (request.status != RequestStatus.Pending) revert InvalidRequestState();

        Task storage task = _tasks[request.taskId];
        request.status = RequestStatus.Rejected;
        task.liveRequestId = 0;

        if (request.kind == RequestKind.Supplement) {
            task.status = TaskStatus.Stopped;
            emit TaskStopped(request.taskId, StopReason.SupplementRejected);
        }

        emit RequestRejected(requestId, request.taskId);
    }

    /// @notice Handles exactly one FIFO entry in constant work. A removed entry is skipped alone;
    ///         an underfunded live head remains in place and blocks every later request.
    function applyNext()
        external
        onlyAuthority
        nonReentrant
        returns (uint256 requestId, ApplyResult result)
    {
        if (queueHead == approvalQueue.length) return (0, ApplyResult.Empty);

        requestId = approvalQueue[queueHead];
        BudgetRequest storage request = _requests[requestId];
        if (request.status == RequestStatus.Removed) {
            ++queueHead;
            emit QueueAdvanced(requestId);
            return (requestId, ApplyResult.Skipped);
        }

        if (request.status != RequestStatus.Queued) revert InvalidRequestState();
        if (request.amount > freeBalance) {
            emit FundingWaiting(requestId, request.amount, freeBalance);
            return (requestId, ApplyResult.Waiting);
        }

        Task storage task = _tasks[request.taskId];
        // Every terminal/queue-exit transition removes the request before it can be applied.
        if (task.status != TaskStatus.Active && task.status != TaskStatus.Stopped) {
            revert InvalidTaskState();
        }

        freeBalance -= request.amount;
        allocatedBalance += request.amount;
        task.available += request.amount;
        task.issued += request.amount;
        task.liveRequestId = 0;
        if (request.kind == RequestKind.Resume) task.status = TaskStatus.Active;

        request.status = RequestStatus.Funded;
        ++queueHead;

        emit BudgetAllocated(requestId, request.taskId, request.amount);
        return (requestId, ApplyResult.Funded);
    }

    function withdraw(uint256 taskId, uint256 amount) external nonReentrant {
        Task storage task = _agentTask(taskId);
        if (task.status != TaskStatus.Active) revert InvalidTaskState();
        if (task.paused) revert TaskIsPaused();
        _positive(amount);
        if (amount > task.available) revert InsufficientFunds();

        task.started = true;
        task.available -= amount;
        allocatedBalance -= amount;
        asset.sendExact(task.agent, amount);

        emit AgentWithdrawal(taskId, amount);
    }

    /// @notice Returns already withdrawn principal. This releases Project funds, never refills
    ///         the task's old withdrawal allowance, and cannot revise a completed task's expense.
    function returnFunds(uint256 taskId, uint256 amount) external nonReentrant {
        Task storage task = _agentTask(taskId);
        _requireNonterminal(task);
        _positive(amount);
        if (amount > _externalOutstanding(task)) revert ExcessReturn();

        task.recovered += amount;
        freeBalance += amount;
        asset.receiveExact(task.agent, amount);

        emit ExternalReturn(taskId, amount);
    }

    function release(uint256 taskId, uint256 amount) external nonReentrant {
        Task storage task = _agentTask(taskId);
        _requireNonterminal(task);

        _release(task, amount);

        emit ControlledRelease(taskId, amount);
    }

    /// @notice Completes a previously funded task after explicit release of controlled remainder.
    /// @dev Any pending/queued request is removed atomically so it cannot fund a completed task.
    function finalize(uint256 taskId, bytes32 reportRef) external nonReentrant {
        Task storage task = _agentTask(taskId);
        _requireNonterminal(task);
        if (task.issued == 0) revert TaskNotFunded();
        if (task.available != 0) revert ControlledRemainder();

        _removeRequest(task, RemovalReason.Completed);
        task.status = TaskStatus.Completed;
        task.reportRef = reportRef;

        emit TaskFinalized(taskId, task.issued - task.recovered, reportRef);
    }

    function cancel(uint256 taskId) external onlyOwner nonReentrant {
        Task storage task = _task(taskId);
        _requireNonterminal(task);
        if (task.started) revert TaskAlreadyStarted();

        _removeRequest(task, RemovalReason.Canceled);
        if (task.available != 0) _release(task, task.available);
        task.status = TaskStatus.Canceled;

        emit TaskCanceled(taskId);
    }

    function markOutOfBudget(uint256 taskId) external onlyAuthority nonReentrant {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Active && task.status != TaskStatus.Stopped) {
            revert InvalidTaskState();
        }

        _removeRequest(task, RemovalReason.OutOfBudget);
        task.status = TaskStatus.OutOfBudget;

        emit TaskStopped(taskId, StopReason.AuthorityOutOfBudget);
        emit TaskOutOfBudget(taskId);
    }

    function pause(uint256 taskId) external onlyOwner nonReentrant {
        Task storage task = _task(taskId);
        _requireNonterminal(task);
        if (task.paused) revert TaskIsPaused();

        task.paused = true;

        emit TaskPaused(taskId);
    }

    function reclaim(uint256 taskId, uint256 amount) external onlyOwner nonReentrant {
        Task storage task = _task(taskId);
        _requireNonterminal(task);
        if (!task.paused) revert TaskIsNotPaused();

        _release(task, amount);

        emit BudgetReclaimed(taskId, amount);
    }

    /// @notice Authority queues new principal for an already-funded, paused active task.
    function restore(uint256 taskId, uint256 amount, bytes32 purposeRef)
        external
        onlyAuthority
        nonReentrant
        returns (uint256 requestId)
    {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Active) revert InvalidTaskState();
        if (!task.paused) revert TaskIsNotPaused();
        if (task.issued == 0) revert TaskNotFunded();
        _requestInputs(amount, purposeRef);

        requestId = _newRequest(taskId, task, amount, purposeRef, RequestKind.Restoration);
        _approve(requestId, _requests[requestId]);
    }

    /// @notice Resumes an active paused task using its confirmed available principal.
    /// @dev Retained allocation does not create another issuance or require another approval.
    function continueTask(uint256 taskId) external onlyOwner nonReentrant {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Active) revert InvalidTaskState();
        if (!task.paused) revert TaskIsNotPaused();
        if (task.available == 0) revert InsufficientFunds();

        task.paused = false;

        emit TaskContinued(taskId);
    }

    function requestReplenishment(uint256 amount, bytes32 noteRef)
        external
        onlyAuthority
        nonReentrant
        returns (uint256 invoiceId)
    {
        _positive(amount);

        invoiceId = ++invoiceCount;
        _invoices[invoiceId] = Invoice({
            requested: amount,
            requestRef: noteRef,
            status: InvoiceStatus.Open,
            intendedContribution: 0,
            responseRef: bytes32(0),
            funded: 0
        });

        emit ReplenishmentRequested(invoiceId, amount, noteRef);
    }

    /// @notice Closes the invoice independently of any actual token transfer.
    function respondReplenishment(uint256 invoiceId, uint256 intendedContribution, bytes32 noteRef)
        external
        onlyOwner
        nonReentrant
    {
        Invoice storage invoice = _invoice(invoiceId);
        if (invoice.status != InvoiceStatus.Open) revert InvalidInvoiceState();

        invoice.status = InvoiceStatus.Closed;
        invoice.intendedContribution = intendedContribution;
        invoice.responseRef = noteRef;

        emit ReplenishmentResponded(invoiceId, intendedContribution, noteRef);
    }

    function getTask(uint256 taskId) external view returns (Task memory) {
        return _task(taskId);
    }

    function getRequest(uint256 requestId) external view returns (BudgetRequest memory) {
        return _request(requestId);
    }

    function getInvoice(uint256 invoiceId) external view returns (Invoice memory) {
        return _invoice(invoiceId);
    }

    function queueLength() external view returns (uint256) {
        return approvalQueue.length;
    }

    function accountedBalance() external view returns (uint256) {
        return freeBalance + allocatedBalance;
    }

    function externalOutstanding(uint256 taskId) external view returns (uint256) {
        return _externalOutstanding(_task(taskId));
    }

    function taskExpense(uint256 taskId) external view returns (uint256) {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Completed) revert InvalidTaskState();

        return task.issued - task.recovered;
    }

    function _newRequest(
        uint256 taskId,
        Task storage task,
        uint256 amount,
        bytes32 purposeRef,
        RequestKind kind
    ) private returns (uint256 requestId) {
        if (task.liveRequestId != 0) revert LiveRequestExists();

        requestId = ++requestCount;
        _requests[requestId] = BudgetRequest({
            taskId: taskId,
            amount: amount,
            purposeRef: purposeRef,
            status: RequestStatus.Pending,
            kind: kind
        });
        task.liveRequestId = requestId;

        emit BudgetRequested(requestId, taskId, amount, purposeRef, kind);
    }

    function _approve(uint256 requestId, BudgetRequest storage request) private {
        request.status = RequestStatus.Queued;
        approvalQueue.push(requestId);

        emit RequestApproved(requestId, approvalQueue.length);
    }

    function _removeRequest(Task storage task, RemovalReason reason) private {
        uint256 requestId = task.liveRequestId;
        if (requestId == 0) return;

        _requests[requestId].status = RequestStatus.Removed;
        task.liveRequestId = 0;

        emit RequestRemoved(requestId, reason);
    }

    function _release(Task storage task, uint256 amount) private {
        _positive(amount);
        if (amount > task.available) revert InsufficientFunds();

        task.available -= amount;
        task.recovered += amount;
        allocatedBalance -= amount;
        freeBalance += amount;
    }

    function _agentTask(uint256 taskId) private view returns (Task storage task) {
        task = _task(taskId);
        if (msg.sender != task.agent) revert Unauthorized();
    }

    function _task(uint256 taskId) private view returns (Task storage task) {
        task = _tasks[taskId];
        if (task.status == TaskStatus.None) revert UnknownTask();
    }

    function _request(uint256 requestId) private view returns (BudgetRequest storage request) {
        request = _requests[requestId];
        if (request.status == RequestStatus.None) revert UnknownRequest();
    }

    function _invoice(uint256 invoiceId) private view returns (Invoice storage invoice) {
        invoice = _invoices[invoiceId];
        if (invoice.status == InvoiceStatus.None) revert UnknownInvoice();
    }

    function _externalOutstanding(Task storage task) private view returns (uint256) {
        return task.issued - task.recovered - task.available;
    }

    function _requireNonterminal(Task storage task) private view {
        if (task.status == TaskStatus.Completed || task.status == TaskStatus.Canceled) {
            revert InvalidTaskState();
        }
    }

    function _requestInputs(uint256 amount, bytes32 purposeRef) private pure {
        _positive(amount);
        if (purposeRef == bytes32(0)) revert MissingPurpose();
    }

    function _positive(uint256 amount) private pure {
        if (amount == 0) revert InvalidAmount();
    }
}
