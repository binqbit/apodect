// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.33;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { IERC20Metadata } from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { ProjectDefinitions } from "./ProjectDefinitions.sol";
import { ExactToken } from "./ExactToken.sol";
import { Coordination as C } from "./Coordination.sol";
import { RouteEscrow } from "./RouteEscrow.sol";

/// @title Apodect locally funded Project
/// @notice Holds one Project's token principal and enforces its local task lifecycle.
/// @dev A fresh owner-bound Project may permanently opt into NEAR coordination. It has no upgrade
///      entry point, delegated token approvals, or arbitrary external-call facility.
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

    bytes32 public coordinationDomain;
    address public coordinatorSigner;
    uint256 public heldBalance;
    uint256 public checkpointSequence;
    uint256 public fundingSequence;
    mapping(bytes32 planId => C.Plan) private _plans;
    mapping(bytes32 routeId => C.Route) private _deliveries;
    mapping(bytes32 routeId => C.Route) private _refunds;
    mapping(bytes32 operation => C.Outcome) private _outcomes;
    mapping(uint256 taskId => uint64) public controlNonce;
    mapping(bytes32 observation => bool) public checkpointUsed;

    error CoordinatedProject();
    error CoordinationNotBound();
    error InvalidCoordination();
    error InvalidCommand();
    error CommandExpired();
    error CommandConflict();
    error CommandInvalidated();
    error InvalidPlan();
    error InvalidRoute();
    error StaleControl();
    error AmountOverflow();

    event CoordinationBound(bytes32 indexed domain, address indexed signer);
    event CommandExecuted(
        bytes32 indexed operation, bytes32 digest, uint256 action, uint256 result
    );
    event CommandTombstoned(bytes32 indexed operation);
    event PlanChanged(bytes32 indexed plan, C.PlanStatus status, uint256 held);
    event RouteRegistered(
        bytes32 indexed route, bool refund, bytes32 plan, address escrow, uint256 amount
    );
    event RouteCollected(bytes32 indexed route, bool refund, uint256 amount, uint256 total);
    event RouteDispatched(
        bytes32 indexed route, bytes32 indexed plan, address recipient, uint256 amount
    );
    event CheckpointRecorded(bytes32 indexed observation, bytes32 indexed domain, bytes payload);

    event FundingRecorded(bytes32 indexed fundingId, bytes32 indexed domain, bytes payload);

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

    modifier standalone() {
        if (coordinationDomain != bytes32(0)) revert CoordinatedProject();
        _;
    }

    /// @notice Changes future authority actions without invalidating existing work or invoices.
    function setAuthority(address newAuthority) external onlyOwner standalone nonReentrant {
        if (newAuthority == address(0)) revert InvalidAddress();

        address previous = authority;
        authority = newAuthority;

        emit AuthorityChanged(previous, newAuthority);
    }

    /// @notice Funds this Project from its owner. An optional invoice must already be closed.
    /// @dev Only the actual exact token transfer credits cash; direct donations remain unattributed.
    function fund(uint256 amount, uint256 invoiceId) external onlyOwner nonReentrant {
        _positive(amount);
        _checkCapacity(amount);
        if (coordinationDomain != bytes32(0) && invoiceId != 0) revert CoordinatedProject();
        if (invoiceId != 0) {
            Invoice storage invoice = _invoice(invoiceId);
            if (invoice.status != InvoiceStatus.Closed) revert InvalidInvoiceState();
            invoice.funded += amount;
        }

        freeBalance += amount;
        asset.receiveExact(owner, amount);

        emit ProjectFunded(amount, invoiceId);
        if (coordinationDomain != bytes32(0)) _recordFunding(amount, bytes32(0));
    }

    /// @notice Attributed owner contribution for a NEAR invoice; invoice validation occurs on NEAR.
    function coordinatedFund(uint256 amount, bytes32 invoiceId) external onlyOwner nonReentrant {
        if (coordinationDomain == bytes32(0)) revert CoordinationNotBound();
        _positive(amount);
        _checkCapacity(amount);
        freeBalance += amount;
        asset.receiveExact(owner, amount);
        emit ProjectFunded(amount, 0);
        _recordFunding(amount, invoiceId);
    }

    function _recordFunding(uint256 amount, bytes32 invoiceId) private {
        bytes32 fundingId = bytes32(++fundingSequence);
        emit FundingRecorded(
            fundingId,
            coordinationDomain,
            abi.encode(
                coordinationDomain, fundingId, invoiceId, amount, bytes32(uint256(uint160(owner)))
            )
        );
    }

    function withdrawFree(uint256 amount) external onlyOwner standalone nonReentrant {
        _withdrawFree(amount);
    }

    function _withdrawFree(uint256 amount) private {
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

    function approveRequest(uint256 requestId) external onlyAuthority standalone nonReentrant {
        BudgetRequest storage request = _request(requestId);
        if (request.status != RequestStatus.Pending) revert InvalidRequestState();

        _approve(requestId, request);
    }

    function rejectRequest(uint256 requestId) external onlyAuthority standalone nonReentrant {
        _reject(requestId);
    }

    function _reject(uint256 requestId) private {
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
        standalone
        nonReentrant
        returns (uint256 requestId, ApplyResult result)
    {
        if (queueHead == approvalQueue.length) {
            return (0, ApplyResult.Empty);
        }

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
        _checkCapacity(amount);

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

    function cancel(uint256 taskId) external onlyOwner standalone nonReentrant {
        _cancel(taskId);
    }

    function _cancel(uint256 taskId) private {
        Task storage task = _task(taskId);
        _requireNonterminal(task);
        if (task.started) revert TaskAlreadyStarted();

        _removeRequest(task, RemovalReason.Canceled);
        if (task.available != 0) _release(task, task.available);
        task.status = TaskStatus.Canceled;

        emit TaskCanceled(taskId);
    }

    function markOutOfBudget(uint256 taskId) external onlyAuthority standalone nonReentrant {
        _outOfBudget(taskId);
    }

    function _outOfBudget(uint256 taskId) private {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Active && task.status != TaskStatus.Stopped) {
            revert InvalidTaskState();
        }

        _removeRequest(task, RemovalReason.OutOfBudget);
        task.status = TaskStatus.OutOfBudget;

        emit TaskStopped(taskId, StopReason.AuthorityOutOfBudget);
        emit TaskOutOfBudget(taskId);
    }

    function pause(uint256 taskId) external onlyOwner standalone nonReentrant {
        _pause(taskId);
    }

    function _pause(uint256 taskId) private {
        Task storage task = _task(taskId);
        _requireNonterminal(task);
        if (task.paused) revert TaskIsPaused();

        task.paused = true;

        emit TaskPaused(taskId);
    }

    function reclaim(uint256 taskId, uint256 amount) external onlyOwner standalone nonReentrant {
        _reclaim(taskId, amount);
    }

    function _reclaim(uint256 taskId, uint256 amount) private {
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
        standalone
        nonReentrant
        returns (uint256 requestId)
    {
        requestId = _restoreRequest(taskId, amount, purposeRef);
        _approve(requestId, _requests[requestId]);
    }

    function _restoreRequest(uint256 taskId, uint256 amount, bytes32 purposeRef)
        private
        returns (uint256 requestId)
    {
        Task storage task = _task(taskId);
        if (task.status != TaskStatus.Active) revert InvalidTaskState();
        if (!task.paused) revert TaskIsNotPaused();
        if (task.issued == 0) revert TaskNotFunded();
        _requestInputs(amount, purposeRef);

        requestId = _newRequest(taskId, task, amount, purposeRef, RequestKind.Restoration);
    }

    /// @notice Resumes an active paused task using its confirmed available principal.
    /// @dev Retained allocation does not create another issuance or require another approval.
    function continueTask(uint256 taskId) external onlyOwner standalone nonReentrant {
        _continue(taskId);
    }

    function _continue(uint256 taskId) private {
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
        standalone
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
        standalone
        nonReentrant
    {
        Invoice storage invoice = _invoice(invoiceId);
        if (invoice.status != InvoiceStatus.Open) revert InvalidInvoiceState();

        invoice.status = InvoiceStatus.Closed;
        invoice.intendedContribution = intendedContribution;
        invoice.responseRef = noteRef;

        emit ReplenishmentResponded(invoiceId, intendedContribution, noteRef);
    }

    /// @notice Permanently binds a fresh, empty Project to its coordinator's command key.
    function bindCoordination(bytes32 domain, bytes calldata publicKey)
        external
        onlyOwner
        standalone
        nonReentrant
    {
        if (
            domain == bytes32(0) || publicKey.length != 64 || taskCount != 0 || requestCount != 0
                || invoiceCount != 0 || freeBalance != 0 || allocatedBalance != 0
        ) {
            revert InvalidCoordination();
        }
        uint256 x = uint256(bytes32(publicKey[:32]));
        uint256 y = uint256(bytes32(publicKey[32:]));
        uint256 p = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f;
        if (x >= p || y >= p || mulmod(y, y, p) != addmod(mulmod(mulmod(x, x, p), x, p), 7, p)) {
            revert InvalidCoordination();
        }
        coordinationDomain = domain;
        coordinatorSigner = address(uint160(uint256(keccak256(publicKey))));
        emit CoordinationBound(domain, coordinatorSigner);
    }

    /// @notice Relays one bounded, SHA-256 signed command. A revert does not prove nonexecution.
    /// @dev Successful replay returns the immutable result even after expiry. Tombstones are final.
    function executeCommand(bytes calldata wire, bytes calldata signature, uint8 recoveryId)
        external
        nonReentrant
        returns (uint256 result)
    {
        if (coordinationDomain == bytes32(0)) revert CoordinationNotBound();
        if (wire.length != 384 || signature.length != 64 || recoveryId > 1) {
            revert InvalidCommand();
        }
        C.Command memory command = abi.decode(wire, (C.Command));
        bytes32 digest = sha256(wire);
        if (command.domain != coordinationDomain || command.operation == bytes32(0)) {
            revert InvalidCommand();
        }
        (address recovered, ECDSA.RecoverError error,) = ECDSA.tryRecover(
            digest, recoveryId + 27, bytes32(signature[:32]), bytes32(signature[32:])
        );
        if (error != ECDSA.RecoverError.NoError || recovered != coordinatorSigner) {
            revert Unauthorized();
        }

        C.Outcome storage previous = _outcomes[command.operation];
        if (previous.code == C.OutcomeCode.Invalidated) revert CommandInvalidated();
        if (previous.code != C.OutcomeCode.None) {
            if (previous.digest != digest) revert CommandConflict();
            return previous.value;
        }
        _validateCommand(command);
        // The signed deadline deliberately uses the destination chain timestamp.
        // forge-lint: disable-next-line(block-timestamp)
        if (block.timestamp > command.expires) revert CommandExpired();
        result = _execute(command);
        _outcomes[command.operation] =
            C.Outcome(digest, command.action, C.OutcomeCode.Executed, result);
        emit CommandExecuted(command.operation, digest, command.action, result);
    }

    function _validateCommand(C.Command memory c) private view {
        uint256 action = c.action;
        if (action == 0 || action > uint256(C.Action.Invalidate) || c.limit != 0) {
            revert InvalidCommand();
        }
        bool requiresPlan = action <= uint256(C.Action.ClosePlan);
        bool requiresTask = action == uint256(C.Action.Allocate)
            || (action >= uint256(C.Action.Reject) && action <= uint256(C.Action.RestoreRequest));
        bool requiresRequest =
            action == uint256(C.Action.Allocate) || action == uint256(C.Action.Reject);
        bool requiresAmount = action <= uint256(C.Action.Allocate)
            || action == uint256(C.Action.Reclaim) || action == uint256(C.Action.RestoreRequest)
            || action == uint256(C.Action.OwnerWithdraw);
        bool requiresRecipient =
            action == uint256(C.Action.Dispatch) || action == uint256(C.Action.OwnerWithdraw);
        bool hasReference =
            (action >= uint256(C.Action.ExpectDelivery) && action <= uint256(C.Action.Dispatch))
                || action == uint256(C.Action.RestoreRequest)
                || action == uint256(C.Action.Invalidate);
        bool requiresControlNonce =
            action >= uint256(C.Action.Pause) && action <= uint256(C.Action.Cancel);
        if (
            (requiresPlan ? c.related == bytes32(0) : c.related != bytes32(0))
                || (requiresTask ? c.task == 0 : c.task != 0)
                || (requiresRequest ? c.request == 0 : c.request != 0)
                || (!requiresAmount && c.amount != 0)
                || (requiresAmount && action != uint256(C.Action.Reserve) && c.amount == 0)
                || (requiresRecipient ? c.recipient == bytes32(0) : c.recipient != bytes32(0))
                || uint256(c.recipient) >> 160 != 0
                || (hasReference ? c.ref == bytes32(0) : c.ref != bytes32(0))
                || (!requiresControlNonce && c.nonce != 0)
        ) revert InvalidCommand();
        if (
            action == uint256(C.Action.OwnerWithdraw)
                && address(uint160(uint256(c.recipient))) != owner
        ) {
            revert InvalidCommand();
        }
    }

    function _execute(C.Command memory c) private returns (uint256 result) {
        C.Action a = C.Action(c.action);
        if (a == C.Action.Reserve) {
            C.Plan storage plan = _plans[c.related];
            if (plan.status != C.PlanStatus.None) revert InvalidPlan();
            if (c.amount > freeBalance) revert InsufficientFunds();
            freeBalance -= c.amount;
            heldBalance += c.amount;
            plan.status = C.PlanStatus.Open;
            plan.held = c.amount;
            emit PlanChanged(c.related, plan.status, plan.held);
            return c.amount;
        }
        if (a == C.Action.ExpectDelivery || a == C.Action.ExpectRefund) {
            _registerRoute(c, a == C.Action.ExpectRefund);
            return c.amount;
        }
        if (a == C.Action.Dispatch) {
            C.Route storage route = _refunds[c.ref];
            if (
                route.escrow == address(0) || route.plan != c.related || route.expected != c.amount
                    || route.dispatched != 0
            ) revert InvalidRoute();
            _consumeHold(c.related, c.amount);
            route.dispatched = c.amount;
            address recipient = address(uint160(uint256(c.recipient)));
            asset.sendExact(recipient, c.amount);
            emit RouteDispatched(c.ref, c.related, recipient, c.amount);
            return c.amount;
        }
        if (a == C.Action.Allocate) {
            _allocateCoordinated(c);
            return c.amount;
        }
        if (a == C.Action.ClosePlan) {
            C.Plan storage plan = _plans[c.related];
            if (plan.status == C.PlanStatus.None) revert InvalidPlan();
            // An allocated plan is irrevocable here; its actual task rights remain authoritative.
            if (plan.status != C.PlanStatus.Open) return 0;
            result = plan.held;
            heldBalance -= result;
            freeBalance += result;
            plan.held = 0;
            plan.status = C.PlanStatus.Closed;
            emit PlanChanged(c.related, plan.status, 0);
            return result;
        }
        if (a == C.Action.Reject) {
            if (_request(c.request).taskId != c.task) revert InvalidRequestState();
            _reject(c.request);
        } else if (a == C.Action.OutOfBudget) {
            _outOfBudget(c.task);
        } else if (a >= C.Action.Pause && a <= C.Action.Cancel) {
            if (controlNonce[c.task] != c.nonce) revert StaleControl();
            if (a == C.Action.Pause) _pause(c.task);
            else if (a == C.Action.Reclaim) _reclaim(c.task, c.amount);
            else if (a == C.Action.Continue) _continue(c.task);
            else _cancel(c.task);
            controlNonce[c.task] = c.nonce + 1;
            return c.amount;
        } else if (a == C.Action.RestoreRequest) {
            return _restoreRequest(c.task, c.amount, c.ref);
        } else if (a == C.Action.OwnerWithdraw) {
            _withdrawFree(c.amount);
            return c.amount;
        } else if (a == C.Action.Invalidate) {
            if (c.ref == c.operation) revert InvalidCommand();
            C.Outcome storage target = _outcomes[c.ref];
            if (target.code == C.OutcomeCode.None) {
                target.code = C.OutcomeCode.Invalidated;
                emit CommandTombstoned(c.ref);
            }
            return uint256(target.code);
        }
    }

    function _registerRoute(C.Command memory c, bool refund) private {
        if (_plans[c.related].status != C.PlanStatus.Open) revert InvalidPlan();
        C.Route storage route = refund ? _refunds[c.ref] : _deliveries[c.ref];
        if (route.escrow != address(0)) revert InvalidRoute();
        route.plan = c.related;
        route.expected = c.amount;
        route.escrow = address(new RouteEscrow{ salt: keccak256(abi.encode(c.ref, refund)) }(asset));
        emit RouteRegistered(c.ref, refund, c.related, route.escrow, c.amount);
    }

    function _consumeHold(bytes32 planId, uint64 amount) private {
        C.Plan storage plan = _plans[planId];
        if (plan.status != C.PlanStatus.Open) revert InvalidPlan();
        if (plan.held < amount) revert InsufficientFunds();
        plan.held -= amount;
        heldBalance -= amount;
    }

    function _allocateCoordinated(C.Command memory c) private {
        BudgetRequest storage request = _request(c.request);
        Task storage task = _task(c.task);
        if (
            request.taskId != c.task || task.liveRequestId != c.request
                || request.status != RequestStatus.Pending || request.amount != c.amount
        ) revert InvalidRequestState();
        if (
            task.status != TaskStatus.Active
                && !(task.status == TaskStatus.Stopped && request.kind == RequestKind.Resume)
        ) {
            revert InvalidTaskState();
        }
        // Checkpoints share u64 cumulative counters with the coordinator. Recovered
        // cash may be granted again, but must never make this task unrepresentable.
        if (task.issued + c.amount > type(uint64).max) revert AmountOverflow();
        if (_plans[c.related].held != c.amount) revert InsufficientFunds();
        _consumeHold(c.related, c.amount);
        _plans[c.related].status = C.PlanStatus.Allocated;
        allocatedBalance += c.amount;
        task.available += c.amount;
        task.issued += c.amount;
        task.liveRequestId = 0;
        if (request.kind == RequestKind.Resume) task.status = TaskStatus.Active;
        request.status = RequestStatus.Funded;
        emit PlanChanged(c.related, C.PlanStatus.Allocated, 0);
        emit BudgetAllocated(c.request, c.task, c.amount);
    }

    /// @notice Collects real route tokens. Partial delivery waits; partial refunds are cumulative.
    function collectRoute(bytes32 routeId, bool refund)
        external
        nonReentrant
        returns (uint256 amount)
    {
        C.Route storage route = refund ? _refunds[routeId] : _deliveries[routeId];
        if (route.escrow == address(0)) revert InvalidRoute();
        if (refund && route.dispatched == 0) revert InvalidRoute();
        uint256 remaining = route.expected - route.received;
        uint256 balance = asset.balanceOf(route.escrow);
        if (remaining == 0 || balance == 0 || (!refund && balance < remaining)) return 0;
        amount = balance < remaining ? balance : remaining;
        _checkCapacity(amount);
        // amount is bounded by remaining, derived from the route's u64 expected value.
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 collected = uint64(amount);
        route.received += collected;
        C.Plan storage plan = _plans[route.plan];
        if (refund || plan.status == C.PlanStatus.Closed) {
            freeBalance += amount;
        } else {
            if (plan.status != C.PlanStatus.Open) revert InvalidPlan();
            plan.held += collected;
            heldBalance += amount;
        }
        RouteEscrow(route.escrow).collect(amount);
        emit RouteCollected(routeId, refund, amount, route.received);
    }

    function getPlan(bytes32 planId) external view returns (C.Plan memory) {
        return _plans[planId];
    }

    function getRoute(bytes32 routeId, bool refund) external view returns (C.Route memory) {
        return refund ? _refunds[routeId] : _deliveries[routeId];
    }

    function getCommandOutcome(bytes32 operation) external view returns (C.Outcome memory) {
        return _outcomes[operation];
    }

    /// @notice Publishes immutable evidence of current local effects; does not grant any rights.
    function checkpoint(
        bytes32 observation,
        uint256 taskId,
        uint256 requestId,
        bytes32 planId,
        bytes32 routeId,
        bool refund,
        bytes32 operation
    ) external nonReentrant returns (bytes memory payload) {
        if (coordinationDomain == bytes32(0)) revert CoordinationNotBound();
        if (observation == bytes32(0) || checkpointUsed[observation]) revert InvalidCommand();
        checkpointUsed[observation] = true;
        bytes32[39] memory words;
        words[0] = coordinationDomain;
        words[1] = observation;
        words[2] = bytes32(++checkpointSequence);
        words[3] = bytes32(block.number);
        words[4] = bytes32(block.timestamp);
        words[5] = bytes32(freeBalance);
        words[6] = bytes32(heldBalance);
        Task storage task = _tasks[taskId];
        words[7] = bytes32(taskId);
        words[8] = bytes32(task.status == TaskStatus.None ? uint256(255) : uint256(task.status) - 1);
        words[9] = bytes32(uint256(task.paused ? 1 : 0));
        words[10] = bytes32(uint256(task.started ? 1 : 0));
        words[11] = bytes32(task.available);
        words[12] = bytes32(task.issued);
        words[13] = bytes32(task.recovered);
        // EVM transfers settle synchronously; pending_outflow (word 14) stays zero.
        words[15] = bytes32(uint256(controlNonce[taskId]));
        words[16] = bytes32(uint256(uint160(task.agent)));
        BudgetRequest storage request = _requests[requestId];
        words[17] = bytes32(requestId);
        words[18] = bytes32(request.taskId);
        words[19] = bytes32(request.amount);
        words[20] = bytes32(uint256(request.kind));
        words[21] = bytes32(
            request.status == RequestStatus.None ? uint256(255) : uint256(request.status) - 1
        );
        words[22] = request.purposeRef;
        words[23] = planId;
        words[24] = bytes32(uint256(_plans[planId].status));
        words[25] = bytes32(uint256(_plans[planId].held));
        C.Route storage route = refund ? _refunds[routeId] : _deliveries[routeId];
        words[26] = routeId;
        words[27] = route.plan;
        words[28] =
            bytes32(route.escrow == address(0) ? uint256(0) : refund ? uint256(2) : uint256(1));
        words[29] = bytes32(uint256(route.expected));
        words[30] = bytes32(uint256(route.received));
        words[31] = bytes32(uint256(route.dispatched));
        C.Outcome storage outcome = _outcomes[operation];
        words[32] = operation;
        words[33] = outcome.digest;
        words[34] = bytes32(outcome.action);
        words[35] = bytes32(uint256(outcome.code));
        words[36] = bytes32(outcome.value);
        words[37] = bytes32(task.liveRequestId);
        words[38] = bytes32(uint256(task.liveRequestId == 0 ? 0 : 1));
        payload = abi.encode(words);
        emit CheckpointRecorded(observation, coordinationDomain, payload);
    }

    function _checkCapacity(uint256 increase) private view {
        if (
            coordinationDomain != bytes32(0)
                && (increase > type(uint64).max
                    || freeBalance + allocatedBalance + heldBalance + increase > type(uint64).max)
        ) {
            revert AmountOverflow();
        }
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
        return freeBalance + allocatedBalance + heldBalance;
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

    function _requestInputs(uint256 amount, bytes32 purposeRef) private view {
        _positive(amount);
        if (coordinationDomain != bytes32(0) && amount > type(uint64).max) revert AmountOverflow();
        if (purposeRef == bytes32(0)) revert MissingPurpose();
    }

    function _positive(uint256 amount) private pure {
        if (amount == 0) revert InvalidAmount();
    }
}
