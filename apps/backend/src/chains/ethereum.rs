use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, LogData, TxKind, U256, keccak256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, SolEvent, SolValue, sol};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::rpc::Rpc;

sol! {
    #[derive(Debug, Serialize)]
    struct Task {
        address agent;
        uint8 status;
        bool paused;
        bool started;
        uint256 available;
        uint256 issued;
        uint256 recovered;
        uint256 liveRequestId;
        bytes32 reportRef;
    }

    #[derive(Debug, Serialize)]
    struct BudgetRequest {
        uint256 taskId;
        uint256 amount;
        bytes32 purposeRef;
        uint8 status;
        uint8 kind;
    }

    #[derive(Debug, Serialize)]
    struct Plan { uint8 status; uint64 held; }

    #[derive(Debug, Serialize)]
    struct Route {
        bytes32 plan;
        address escrow;
        uint64 expected;
        uint64 received;
        uint64 dispatched;
    }

    #[derive(Debug, Serialize)]
    struct Outcome { bytes32 digest; uint256 action; uint8 code; uint256 value; }

    function owner() external view returns (address);
    function authority() external view returns (address);
    function asset() external view returns (address);
    function coordinationDomain() external view returns (bytes32);
    function coordinatorSigner() external view returns (address);
    function freeBalance() external view returns (uint256);
    function allocatedBalance() external view returns (uint256);
    function heldBalance() external view returns (uint256);
    function taskCount() external view returns (uint256);
    function requestCount() external view returns (uint256);
    function fundingSequence() external view returns (uint256);
    function checkpointSequence() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function decimals() external view returns (uint8);
    function getTask(uint256 taskId) external view returns (Task);
    function getRequest(uint256 requestId) external view returns (BudgetRequest);
    function controlNonce(uint256 taskId) external view returns (uint64);
    function getPlan(bytes32 planId) external view returns (Plan);
    function getRoute(bytes32 routeId, bool refund) external view returns (Route);
    function predictRouteAddress(bytes32 routeId, bool refund) external view returns (address);
    function getCommandOutcome(bytes32 operation) external view returns (Outcome);
    function executeCommand(bytes wire, bytes signature, uint8 recoveryId) external returns (uint256);
    function collectRoute(bytes32 routeId, bool refund) external returns (uint256);
    function checkpoint(
        bytes32 observation, uint256 taskId, uint256 requestId, bytes32 planId,
        bytes32 routeId, bool refund, bytes32 operation
    ) external returns (bytes);

    event ProjectCreated(
        uint256 indexed workspaceId, uint256 indexed projectNonce,
        address indexed project, address authority
    );
    event CheckpointRecorded(bytes32 indexed observation, bytes32 indexed domain, bytes payload);
    event FundingRecorded(bytes32 indexed fundingId, bytes32 indexed domain, bytes payload);
}

/// Every read in a projection uses the same finalized block, including ERC-20 balances.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FinalizedBlock {
    pub hash: B256,
    pub height: u64,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct ProjectSnapshot {
    pub owner: Address,
    pub authority: Address,
    pub token: Address,
    pub domain: B256,
    pub coordinator_signer: Address,
    pub free: U256,
    pub allocated: U256,
    pub held: U256,
    pub token_balance: U256,
    pub task_count: U256,
    pub request_count: U256,
    pub funding_sequence: U256,
    pub checkpoint_sequence: U256,
}

#[derive(Debug, Serialize)]
pub struct TaskSnapshot {
    pub task: Task,
    pub control_nonce: u64,
}

#[derive(Debug, Serialize)]
pub struct RouteSnapshot {
    pub route: Route,
    pub escrow: Address,
    pub token_balance: U256,
}

#[derive(Debug, Serialize)]
pub struct DiscoveredProject {
    pub workspace_id: U256,
    pub project_nonce: U256,
    pub project: Address,
    pub authority: Address,
    pub transaction_hash: B256,
    pub block_hash: B256,
    pub block_height: u64,
    pub log_index: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CheckpointRequest {
    pub observation: B256,
    pub task: U256,
    pub request: U256,
    pub plan: B256,
    pub route: B256,
    pub refund: bool,
    pub operation: B256,
}

/// The gas payer can only relay these permissionless vault methods.
#[derive(Debug)]
pub enum RelayCall {
    Execute {
        command: Bytes,
        signature: Bytes,
        recovery_id: u8,
    },
    Checkpoint(CheckpointRequest),
    Collect {
        route: B256,
        refund: bool,
    },
}

impl RelayCall {
    fn encode(&self) -> Result<Bytes> {
        Ok(match self {
            Self::Execute {
                command,
                signature,
                recovery_id,
            } => {
                ensure!(
                    command.len() == 384,
                    "command must contain twelve ABI words"
                );
                ensure!(
                    signature.len() == 64 && *recovery_id <= 1,
                    "invalid coordinator signature"
                );
                executeCommandCall {
                    wire: command.clone(),
                    signature: signature.clone(),
                    recoveryId: *recovery_id,
                }
                .abi_encode()
            }
            Self::Checkpoint(request) => {
                ensure!(
                    request.observation != B256::ZERO,
                    "checkpoint observation is zero"
                );
                checkpointCall {
                    observation: request.observation,
                    taskId: request.task,
                    requestId: request.request,
                    planId: request.plan,
                    routeId: request.route,
                    refund: request.refund,
                    operation: request.operation,
                }
                .abi_encode()
            }
            Self::Collect { route, refund } => {
                ensure!(*route != B256::ZERO, "route identity is zero");
                collectRouteCall {
                    routeId: *route,
                    refund: *refund,
                }
                .abi_encode()
            }
        }
        .into())
    }
}

/// Persist this entire record before calling `broadcast`. Retrying uses these exact bytes.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreparedTransaction {
    pub hash: B256,
    pub raw: Bytes,
    pub sender: Address,
    pub nonce: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct GasLimits {
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventEvidence {
    pub payload: Bytes,
    pub data: Bytes,
    pub emitter: Address,
    pub topics: Vec<B256>,
    pub transaction_hash: B256,
    pub block_hash: B256,
    pub block_height: u64,
    /// Ethereum's block-global log index, never the position in a receipt's logs array.
    pub log_index: u64,
    pub transaction_index: u64,
}

impl EventEvidence {
    pub fn mpc_evidence(&self) -> (Value, Vec<Value>) {
        let request = json!({"Ethereum": {
            "tx_id": hex::encode(self.transaction_hash), "finality": "Finalized",
            "extractors": [{"Log": {"log_index": self.log_index}}],
        }});
        let values = vec![json!({"EvmExtractedValue": {"Log": {
            "removed": false, "log_index": self.log_index,
            "transaction_index": self.transaction_index,
            "transaction_hash": hex::encode(self.transaction_hash),
            "block_hash": hex::encode(self.block_hash), "block_number": self.block_height,
            "address": hex::encode(self.emitter), "data": self.data,
            "topics": self.topics.iter().map(hex::encode).collect::<Vec<_>>(),
        }}})];
        (request, values)
    }
}

#[derive(Clone, Debug)]
pub struct FinalizedReceipt {
    pub hash: B256,
    pub block: FinalizedBlock,
    pub sender: Address,
    pub destination: Option<Address>,
    pub succeeded: bool,
    logs: Vec<RpcLog>,
}

impl FinalizedReceipt {
    pub fn checkpoint_evidence(
        &self,
        project: Address,
        domain: B256,
        observation: B256,
    ) -> Result<EventEvidence> {
        self.evidence(project, domain, observation, EvidenceKind::Checkpoint)
    }

    pub fn funding_evidence(
        &self,
        project: Address,
        domain: B256,
        funding: B256,
    ) -> Result<EventEvidence> {
        self.evidence(project, domain, funding, EvidenceKind::Funding)
    }

    fn evidence(
        &self,
        project: Address,
        domain: B256,
        identity: B256,
        kind: EvidenceKind,
    ) -> Result<EventEvidence> {
        ensure!(
            self.succeeded,
            "reverted transaction cannot provide evidence"
        );
        let mut matches = self
            .logs
            .iter()
            .filter(|log| log.address == project && log.topics == [kind.topic(), identity, domain]);
        let log = matches
            .next()
            .context("vault evidence missing from finalized receipt")?;
        ensure!(
            matches.next().is_none(),
            "ambiguous immutable vault evidence"
        );
        decode_evidence(log, project, domain, identity, kind, &self.block)
    }
}

#[derive(Clone)]
pub struct Ethereum {
    rpc: Rpc,
    chain_id: u64,
    genesis_hash: B256,
}

impl Ethereum {
    /// Both identities are deployment configuration, not values trusted from the RPC endpoint.
    pub fn new(rpc: Rpc, chain_id: u64, genesis_hash: B256) -> Self {
        Self {
            rpc,
            chain_id,
            genesis_hash,
        }
    }

    pub async fn verify_network(&self) -> Result<()> {
        let chain_id: U256 = self.rpc.call("eth_chainId", &json!([])).await?;
        ensure!(
            chain_id == U256::from(self.chain_id),
            "Ethereum chain ID differs from configuration"
        );
        let genesis = self.block("0x0").await?;
        ensure!(
            genesis.hash == self.genesis_hash,
            "Ethereum genesis differs from configuration"
        );
        Ok(())
    }

    pub async fn finalized_block(&self) -> Result<FinalizedBlock> {
        self.block("finalized").await
    }

    pub async fn finalized_block_at(&self, height: u64) -> Result<FinalizedBlock> {
        ensure!(
            height <= self.finalized_block().await?.height,
            "requested block is not finalized"
        );
        self.block(&quantity(height)).await
    }

    pub async fn project(
        &self,
        project: Address,
        block: &FinalizedBlock,
    ) -> Result<ProjectSnapshot> {
        let token = self.read(project, assetCall {}, block).await?;
        ensure!(
            self.read(token, decimalsCall {}, block).await? == 6,
            "project asset must use six decimals"
        );

        let snapshot = ProjectSnapshot {
            owner: self.read(project, ownerCall {}, block).await?,
            authority: self.read(project, authorityCall {}, block).await?,
            token,
            domain: self.read(project, coordinationDomainCall {}, block).await?,
            coordinator_signer: self.read(project, coordinatorSignerCall {}, block).await?,
            free: self.read(project, freeBalanceCall {}, block).await?,
            allocated: self.read(project, allocatedBalanceCall {}, block).await?,
            held: self.read(project, heldBalanceCall {}, block).await?,
            token_balance: self.balance(token, project, block).await?,
            task_count: self.read(project, taskCountCall {}, block).await?,
            request_count: self.read(project, requestCountCall {}, block).await?,
            funding_sequence: self.read(project, fundingSequenceCall {}, block).await?,
            checkpoint_sequence: self.read(project, checkpointSequenceCall {}, block).await?,
        };
        let accounted = snapshot
            .free
            .checked_add(snapshot.allocated)
            .and_then(|value| value.checked_add(snapshot.held))
            .context("project accounting overflow")?;
        ensure!(
            snapshot.token_balance >= accounted,
            "project token custody is below attributed balances"
        );
        Ok(snapshot)
    }

    pub async fn task(
        &self,
        project: Address,
        id: U256,
        block: &FinalizedBlock,
    ) -> Result<TaskSnapshot> {
        let task = self
            .read(project, getTaskCall { taskId: id }, block)
            .await?;
        ensure!((1..=5).contains(&task.status), "unknown task state");
        Ok(TaskSnapshot {
            task,
            control_nonce: self
                .read(project, controlNonceCall { taskId: id }, block)
                .await?,
        })
    }

    pub async fn request(
        &self,
        project: Address,
        id: U256,
        block: &FinalizedBlock,
    ) -> Result<BudgetRequest> {
        let request = self
            .read(project, getRequestCall { requestId: id }, block)
            .await?;
        ensure!(
            (1..=5).contains(&request.status) && request.kind <= 3,
            "unknown budget request state"
        );
        Ok(request)
    }

    pub async fn plan(&self, project: Address, id: B256, block: &FinalizedBlock) -> Result<Plan> {
        let plan = self
            .read(project, getPlanCall { planId: id }, block)
            .await?;
        ensure!(plan.status <= 3, "unknown plan state");
        Ok(plan)
    }

    pub async fn outcome(
        &self,
        project: Address,
        operation: B256,
        block: &FinalizedBlock,
    ) -> Result<Outcome> {
        let outcome = self
            .read(project, getCommandOutcomeCall { operation }, block)
            .await?;
        ensure!(
            outcome.code <= 4 && outcome.action <= U256::from(15),
            "unknown command outcome"
        );
        Ok(outcome)
    }

    pub async fn route(
        &self,
        project: Address,
        id: B256,
        refund: bool,
        block: &FinalizedBlock,
    ) -> Result<RouteSnapshot> {
        let route = self
            .read(
                project,
                getRouteCall {
                    routeId: id,
                    refund,
                },
                block,
            )
            .await?;
        let escrow = self
            .read(
                project,
                predictRouteAddressCall {
                    routeId: id,
                    refund,
                },
                block,
            )
            .await?;
        ensure!(
            route.escrow == Address::ZERO || route.escrow == escrow,
            "route escrow differs from its deterministic address"
        );
        let token = self.read(project, assetCall {}, block).await?;
        Ok(RouteSnapshot {
            route,
            escrow,
            token_balance: self.balance(token, escrow, block).await?,
        })
    }

    pub async fn balance(
        &self,
        token: Address,
        account: Address,
        block: &FinalizedBlock,
    ) -> Result<U256> {
        self.read(token, balanceOfCall { account }, block).await
    }

    pub async fn allowance(
        &self,
        token: Address,
        owner: Address,
        spender: Address,
        block: &FinalizedBlock,
    ) -> Result<U256> {
        self.read(token, allowanceCall { owner, spender }, block)
            .await
    }

    /// The caller persists its block cursor after applying the returned discoveries atomically.
    pub async fn discover_projects(
        &self,
        registry: Address,
        from: u64,
        through: &FinalizedBlock,
    ) -> Result<Vec<DiscoveredProject>> {
        ensure!(
            from <= through.height && through.height - from < 2_000,
            "discovery page must contain 1..=2000 blocks"
        );
        self.require_canonical(through).await?;
        let logs: Vec<RpcLog> = self.rpc.call("eth_getLogs", &json!([{
            "address": registry, "fromBlock": quantity(from), "toBlock": quantity(through.height),
            "topics": [ProjectCreated::SIGNATURE_HASH],
        }])).await?;
        let mut projects = Vec::with_capacity(logs.len());
        for log in logs {
            ensure!(
                log.address == registry && log.height()? >= from && log.height()? <= through.height,
                "registry log does not match discovery filter"
            );
            self.verify_log(&log).await?;
            let event = ProjectCreated::decode_log_data_validate(&log.log_data()?)?;
            projects.push(DiscoveredProject {
                workspace_id: event.workspaceId,
                project_nonce: event.projectNonce,
                project: event.project,
                authority: event.authority,
                transaction_hash: log.transaction_hash,
                block_hash: log.block_hash,
                block_height: log.height()?,
                log_index: log.index()?,
            });
        }
        self.require_canonical(through).await?;
        Ok(projects)
    }

    /// The database must serialize preparation for this signer. Its persisted nonce floor
    /// includes signed transactions that have not yet been acknowledged by the RPC.
    pub async fn next_nonce(&self, sender: Address, persisted_floor: u64) -> Result<u64> {
        let nonce: U256 = self
            .rpc
            .call("eth_getTransactionCount", &json!([sender, "pending"]))
            .await?;
        Ok(u64::try_from(nonce)?.max(persisted_floor))
    }

    pub async fn prepare(
        &self,
        project: Address,
        call: &RelayCall,
        signer: &PrivateKeySigner,
        nonce: u64,
        limits: GasLimits,
    ) -> Result<PreparedTransaction> {
        self.verify_network().await?;
        let input = call.encode()?;
        let sender = signer.address();
        let estimate: U256 = self.rpc.call("eth_estimateGas", &json!([{
            "from": sender, "to": project, "data": input, "value": "0x0", "nonce": quantity(nonce),
        }])).await?;
        let gas_limit = u64::try_from(estimate)?
            .checked_mul(6)
            .and_then(|value| value.checked_add(4))
            .map(|value| value / 5)
            .context("gas estimate overflow")?;
        ensure!(
            gas_limit <= limits.gas_limit,
            "transaction exceeds configured gas limit"
        );

        let latest: RpcBlock = self
            .rpc
            .call::<Option<RpcBlock>>("eth_getBlockByNumber", &json!(["latest", false]))
            .await?
            .context("latest Ethereum block unavailable")?;
        let base_fee = u128::try_from(
            latest
                .base_fee_per_gas
                .context("Ethereum endpoint does not support EIP-1559")?,
        )?;
        let priority: U256 = self
            .rpc
            .call("eth_maxPriorityFeePerGas", &json!([]))
            .await?;
        let priority = u128::try_from(priority)?;
        let max_fee = base_fee
            .checked_mul(2)
            .and_then(|value| value.checked_add(priority))
            .context("gas price overflow")?;
        ensure!(
            max_fee <= limits.max_fee_per_gas,
            "transaction exceeds configured gas price"
        );

        let transaction = TxEip1559 {
            chain_id: self.chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas: max_fee,
            max_priority_fee_per_gas: priority,
            to: TxKind::Call(project),
            value: U256::ZERO,
            input,
            access_list: Default::default(),
        };
        let signature = signer.sign_hash_sync(&transaction.signature_hash())?;
        let raw: Bytes = transaction.into_signed(signature).encoded_2718().into();
        Ok(PreparedTransaction {
            hash: keccak256(&raw),
            raw,
            sender,
            nonce,
        })
    }

    /// An RPC error leaves the outcome unknown. Keep the prepared transaction and reconcile its hash.
    pub async fn broadcast(&self, transaction: &PreparedTransaction) -> Result<()> {
        ensure!(
            keccak256(&transaction.raw) == transaction.hash,
            "stored signed transaction hash mismatch"
        );
        self.verify_network().await?;
        let returned: B256 = self
            .rpc
            .call("eth_sendRawTransaction", &json!([transaction.raw]))
            .await?;
        ensure!(
            returned == transaction.hash,
            "RPC returned a different transaction identity"
        );
        Ok(())
    }

    /// `None` means unknown or not finalized; it never authorizes a replacement transfer.
    pub async fn receipt(&self, hash: B256) -> Result<Option<FinalizedReceipt>> {
        let Some(receipt): Option<RpcReceipt> = self
            .rpc
            .call("eth_getTransactionReceipt", &json!([hash]))
            .await?
        else {
            return Ok(None);
        };
        ensure!(
            receipt.transaction_hash == hash,
            "receipt transaction hash mismatch"
        );
        let height = u64::try_from(receipt.block_number)?;
        if self.finalized_block().await?.height < height {
            return Ok(None);
        }
        let block = self.block(&quantity(height)).await?;
        ensure!(
            block.hash == receipt.block_hash,
            "receipt is outside the canonical finalized chain"
        );
        ensure!(
            receipt.status <= U256::from(1),
            "invalid receipt execution status"
        );
        for log in &receipt.logs {
            ensure!(
                !log.removed
                    && log.transaction_hash == hash
                    && log.block_hash == block.hash
                    && log.transaction_index == receipt.transaction_index
                    && log.height()? == height,
                "receipt contains a log from another inclusion"
            );
        }
        Ok(Some(FinalizedReceipt {
            hash,
            block,
            sender: receipt.from,
            destination: receipt.to,
            succeeded: receipt.status == U256::from(1),
            logs: receipt.logs,
        }))
    }

    pub async fn checkpoint_evidence(
        &self,
        project: Address,
        domain: B256,
        observation: B256,
        from: u64,
        through: &FinalizedBlock,
    ) -> Result<Option<EventEvidence>> {
        self.find_evidence(
            project,
            domain,
            observation,
            EvidenceKind::Checkpoint,
            from,
            through,
        )
        .await
    }

    pub async fn funding_evidence(
        &self,
        project: Address,
        domain: B256,
        funding: B256,
        from: u64,
        through: &FinalizedBlock,
    ) -> Result<Option<EventEvidence>> {
        self.find_evidence(
            project,
            domain,
            funding,
            EvidenceKind::Funding,
            from,
            through,
        )
        .await
    }

    pub async fn list_funding(
        &self,
        project: Address,
        domain: B256,
        from: u64,
        through: &FinalizedBlock,
    ) -> Result<Vec<EventEvidence>> {
        ensure!(
            from <= through.height && through.height - from < 2_000,
            "funding page must contain 1..=2000 blocks"
        );
        self.require_canonical(through).await?;
        let logs: Vec<RpcLog> = self.rpc.call("eth_getLogs", &json!([{
            "address": project, "fromBlock": quantity(from), "toBlock": quantity(through.height),
            "topics": [FundingRecorded::SIGNATURE_HASH, null, domain],
        }])).await?;

        let mut evidence = Vec::with_capacity(logs.len());
        for log in logs {
            ensure!(
                log.height()? >= from && log.height()? <= through.height,
                "event outside requested block range"
            );
            let identity = *log.topics.get(1).context("funding identity missing")?;
            let receipt = self.verify_log(&log).await?;
            evidence.push(decode_evidence(
                &log,
                project,
                domain,
                identity,
                EvidenceKind::Funding,
                &receipt.block,
            )?);
        }
        self.require_canonical(through).await?;
        Ok(evidence)
    }

    async fn find_evidence(
        &self,
        project: Address,
        domain: B256,
        identity: B256,
        kind: EvidenceKind,
        from: u64,
        through: &FinalizedBlock,
    ) -> Result<Option<EventEvidence>> {
        ensure!(
            from <= through.height && through.height - from < 2_000,
            "evidence page must contain 1..=2000 blocks"
        );
        self.require_canonical(through).await?;
        let logs: Vec<RpcLog> = self.rpc.call("eth_getLogs", &json!([{
            "address": project, "fromBlock": quantity(from), "toBlock": quantity(through.height),
            "topics": [kind.topic(), identity, domain],
        }])).await?;
        ensure!(logs.len() <= 1, "ambiguous immutable vault evidence");
        let Some(log) = logs.into_iter().next() else {
            return Ok(None);
        };
        ensure!(
            log.height()? >= from && log.height()? <= through.height,
            "event outside requested block range"
        );
        let receipt = self.verify_log(&log).await?;
        let evidence = decode_evidence(&log, project, domain, identity, kind, &receipt.block)?;
        self.require_canonical(through).await?;
        Ok(Some(evidence))
    }

    async fn verify_log(&self, log: &RpcLog) -> Result<FinalizedReceipt> {
        ensure!(!log.removed, "removed event is not evidence");
        let receipt = self
            .receipt(log.transaction_hash)
            .await?
            .context("event transaction is not finalized")?;
        ensure!(
            receipt.succeeded,
            "reverted transaction cannot provide evidence"
        );
        let matching: Vec<_> = receipt
            .logs
            .iter()
            .filter(|candidate| candidate.log_index == log.log_index)
            .collect();
        ensure!(
            matching.len() == 1 && matching[0] == log,
            "event does not match its receipt at the block-global log index"
        );
        Ok(receipt)
    }

    async fn block(&self, selector: &str) -> Result<FinalizedBlock> {
        let block: Option<RpcBlock> = self
            .rpc
            .call("eth_getBlockByNumber", &json!([selector, false]))
            .await?;
        let block = block.context("Ethereum block unavailable")?;
        Ok(FinalizedBlock {
            hash: block.hash,
            height: u64::try_from(block.number)?,
            timestamp: u64::try_from(block.timestamp)?,
        })
    }

    async fn require_canonical(&self, block: &FinalizedBlock) -> Result<()> {
        ensure!(
            self.finalized_block().await?.height >= block.height,
            "requested block is not finalized"
        );
        let canonical = self.block(&quantity(block.height)).await?;
        ensure!(
            canonical.hash == block.hash && canonical.timestamp == block.timestamp,
            "requested block is not canonical"
        );
        Ok(())
    }

    async fn read<C: SolCall>(
        &self,
        target: Address,
        call: C,
        block: &FinalizedBlock,
    ) -> Result<C::Return> {
        let result: Bytes = self
            .rpc
            .call(
                "eth_call",
                &json!([
                    {"to": target, "data": Bytes::from(call.abi_encode())},
                    {"blockHash": block.hash, "requireCanonical": true},
                ]),
            )
            .await?;
        C::abi_decode_returns_validate(&result).context("invalid vault ABI response")
    }
}

#[derive(Clone, Copy)]
enum EvidenceKind {
    Checkpoint,
    Funding,
}

impl EvidenceKind {
    fn topic(self) -> B256 {
        match self {
            Self::Checkpoint => CheckpointRecorded::SIGNATURE_HASH,
            Self::Funding => FundingRecorded::SIGNATURE_HASH,
        }
    }
}

fn decode_evidence(
    log: &RpcLog,
    project: Address,
    domain: B256,
    identity: B256,
    kind: EvidenceKind,
    block: &FinalizedBlock,
) -> Result<EventEvidence> {
    ensure!(
        !log.removed && log.address == project && log.topics == [kind.topic(), identity, domain],
        "evidence emitter or indexed identity mismatch"
    );
    ensure!(
        log.block_hash == block.hash && log.height()? == block.height,
        "evidence inclusion mismatch"
    );
    let payload = Bytes::abi_decode_validate(&log.data)?;
    ensure!(
        payload.abi_encode() == log.data.as_ref(),
        "noncanonical event payload ABI"
    );
    let words = match kind {
        EvidenceKind::Checkpoint => 39,
        EvidenceKind::Funding => 5,
    };
    ensure!(
        payload.len() == words * 32,
        "incorrect evidence wire length"
    );
    ensure!(
        payload[..32] == domain[..] && payload[32..64] == identity[..],
        "evidence wire identity mismatch"
    );
    if let EvidenceKind::Checkpoint = kind {
        ensure!(
            U256::from_be_slice(&payload[96..128]) == U256::from(block.height)
                && U256::from_be_slice(&payload[128..160]) == U256::from(block.timestamp),
            "checkpoint wire block does not match its finalized inclusion"
        );
    }
    Ok(EventEvidence {
        payload,
        data: log.data.clone(),
        emitter: log.address,
        topics: log.topics.clone(),
        transaction_hash: log.transaction_hash,
        block_hash: log.block_hash,
        block_height: block.height,
        log_index: log.index()?,
        transaction_index: u64::try_from(log.transaction_index)?,
    })
}

fn quantity(value: u64) -> String {
    format!("0x{value:x}")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcBlock {
    hash: B256,
    number: U256,
    timestamp: U256,
    base_fee_per_gas: Option<U256>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcReceipt {
    transaction_hash: B256,
    transaction_index: U256,
    block_hash: B256,
    block_number: U256,
    from: Address,
    to: Option<Address>,
    status: U256,
    logs: Vec<RpcLog>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcLog {
    address: Address,
    topics: Vec<B256>,
    data: Bytes,
    transaction_hash: B256,
    transaction_index: U256,
    block_hash: B256,
    block_number: U256,
    log_index: U256,
    removed: bool,
}

impl RpcLog {
    fn height(&self) -> Result<u64> {
        Ok(u64::try_from(self.block_number)?)
    }
    fn index(&self) -> Result<u64> {
        Ok(u64::try_from(self.log_index)?)
    }
    fn log_data(&self) -> Result<LogData> {
        LogData::new(self.topics.clone(), self.data.clone()).context("invalid event topics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_evidence_rejects_substituted_identity_and_inclusion() {
        let project = Address::repeat_byte(1);
        let domain = B256::repeat_byte(2);
        let observation = B256::repeat_byte(3);
        let block = FinalizedBlock {
            hash: B256::repeat_byte(4),
            height: 19,
            timestamp: 25,
        };
        let mut payload = vec![0; 39 * 32];
        payload[..32].copy_from_slice(domain.as_slice());
        payload[32..64].copy_from_slice(observation.as_slice());
        payload[96..128].copy_from_slice(&U256::from(block.height).to_be_bytes::<32>());
        payload[128..160].copy_from_slice(&U256::from(block.timestamp).to_be_bytes::<32>());
        let log = RpcLog {
            address: project,
            topics: vec![EvidenceKind::Checkpoint.topic(), observation, domain],
            data: Bytes::from(payload).abi_encode().into(),
            transaction_hash: B256::repeat_byte(5),
            transaction_index: U256::from(2),
            block_hash: block.hash,
            block_number: U256::from(block.height),
            log_index: U256::from(17),
            removed: false,
        };
        let decode = |log: &RpcLog| {
            decode_evidence(
                log,
                project,
                domain,
                observation,
                EvidenceKind::Checkpoint,
                &block,
            )
        };
        assert_eq!(decode(&log).unwrap().log_index, 17);
        let mut receipt = FinalizedReceipt {
            hash: log.transaction_hash,
            block: block.clone(),
            sender: Address::repeat_byte(8),
            destination: Some(project),
            succeeded: true,
            logs: vec![log.clone()],
        };
        assert_eq!(
            receipt
                .checkpoint_evidence(project, domain, observation)
                .unwrap()
                .log_index,
            17
        );
        receipt.succeeded = false;
        assert!(
            receipt
                .checkpoint_evidence(project, domain, observation)
                .is_err()
        );
        receipt.succeeded = true;
        receipt.logs.push(log.clone());
        assert!(
            receipt
                .checkpoint_evidence(project, domain, observation)
                .is_err()
        );

        let mut altered = log.clone();
        altered.address = Address::repeat_byte(6);
        assert!(decode(&altered).is_err());
        altered = log.clone();
        altered.topics[1] = B256::repeat_byte(6);
        assert!(decode(&altered).is_err());
        altered = log.clone();
        altered.block_hash = B256::repeat_byte(6);
        assert!(decode(&altered).is_err());
        altered = log.clone();
        altered.removed = true;
        assert!(decode(&altered).is_err());
        altered = log.clone();
        let mut bytes = altered.data.to_vec();
        bytes[64 + 127] ^= 1;
        altered.data = bytes.into();
        assert!(decode(&altered).is_err());
        let mut bytes = log.data.to_vec();
        bytes.extend([0; 32]);
        altered = log.clone();
        altered.data = bytes.into();
        assert!(decode(&altered).is_err());
    }
}
