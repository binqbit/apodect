"""JSON adapter for coordinated Projects on the owned Anvil environment."""

import fcntl
import hashlib
import json

from localnet import CHAIN_ID, CHAIN_MARKER, read_json, require, run

ZERO = "0x" + "00" * 32
CHECKPOINT_FIELDS = """domain observation sequence height timestamp free held
task task_state paused started available issued recovered pending_outflow control_nonce agent
request request_task request_amount request_kind request_state purpose
plan plan_state plan_held route route_plan route_kind route_expected route_received route_dispatched
operation command_digest command_action command_state command_result live_request live_request_exists""".split()
IDENTITIES = {
    "domain",
    "observation",
    "agent",
    "purpose",
    "plan",
    "route",
    "route_plan",
    "operation",
    "command_digest",
    "command_result",
}


def proof(receipt):
    return {
        "tx_hash": receipt["transactionHash"],
        "block_hash": receipt["blockHash"],
        "block_height": str(int(receipt["blockNumber"], 16)),
    }


def word(value):
    value = str(value)
    number = int(value, 16) if value.startswith("0x") else int(value)
    require(0 <= number < 2**256, "Expected a 32-byte unsigned word")
    return f"0x{number:064x}"


class EthereumBridge:
    def __init__(self, local):
        self.local = local
        self.directory = local.directory / "bridge"
        self.directory.mkdir(mode=0o700, exist_ok=True)
        self.manifest = read_json(local.manifest_path)
        self.accounts = read_json(local.accounts_path)
        self.token = self.manifest["asset"]["address"]

    def run(self, request):
        require(isinstance(request, dict), "Expected a JSON object")
        action = request.get("action")
        require(
            action
            in {
                "bootstrap",
                "bind",
                "fund",
                "execute",
                "checkpoint",
                "read_checkpoint",
                "view",
                "route_address",
                "route_deposit",
                "collect",
                "agent",
                "read_funding",
                "relay_account",
            },
            "Unknown Ethereum bridge action",
        )
        with (self.directory / "adapter.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if action == "bootstrap":
                return self.bootstrap(request)
            if action == "relay_account":
                return self.relay_account()
            project = request["project"]
            require(
                self.local.values(project, "asset()")[0] == int(self.token, 16)
                and self.local.values(project, "owner()")[0]
                == int(self.accounts["owner"]["address"], 16),
                "Project is not owned by this local environment",
            )
            return getattr(self, action)(request)

    def relay_account(self):
        """Provision only native test gas for a separate application signer."""
        path = self.local.directory / "keys" / "relayer.json"
        if not path.exists():
            from localnet import write_json

            account = json.loads(run("cast", "wallet", "new", "--json"))[0]
            write_json(path, account)
        account = read_json(path)
        require(
            account["address"].lower()
            not in {value["address"].lower() for value in self.accounts.values()},
            "Application relay must use a separate account",
        )
        balance = int(
            self.local.rpc("eth_getBalance", [account["address"], "latest"]), 16
        )
        if balance < 10 * 10**18:
            self.local.rpc("anvil_setBalance", [account["address"], hex(20 * 10**18)])
        return {"address": account["address"], "key_file": str(path)}

    def bootstrap(self, request):
        # Native bootstrap owns the only Project; coordination binds it in place.
        project = self.manifest["contracts"]["project"]
        registry = self.manifest["contracts"]["registry"]
        if not self.local.values(project, "coordinationDomain()")[0]:
            require(
                all(
                    self.local.values(project, field + "()")[0] == 0
                    for field in (
                        "taskCount",
                        "requestCount",
                        "invoiceCount",
                        "freeBalance",
                        "allocatedBalance",
                        "heldBalance",
                    )
                ),
                "Local Project was already used; refusing to bind existing activity",
            )
        # The random genesis marker survives Anvil state restoration, including pruned block history.
        marker = self.local.rpc("eth_getCode", [CHAIN_MARKER, "latest"])
        return {
            "chain": "ethereum",
            "project": project,
            "vault": project,
            "contracts": {
                "project": project,
                "registry": registry,
                "token": self.token,
            },
            "asset": {"address": self.token, "decimals": 6},
            "asset_id": self.token,
            "accounts": self.accounts,
            "genesis_identity": marker,
            "deposit_receiver": self.accounts["liquidity"]["address"],
            "rpc_url": self.local.url,
            "chain_id": CHAIN_ID,
            "checkpoint_topic": self.event_topic("CheckpointRecorded"),
            "funding_topic": self.event_topic("FundingRecorded"),
            "liquidity_balance": str(
                self.local.values(
                    self.token,
                    "balanceOf(address)",
                    self.accounts["liquidity"]["address"],
                )[0]
            ),
        }

    def bind(self, request):
        project, domain = request["project"], word(request["domain"])
        current = self.local.values(project, "coordinationDomain()")[0]
        public_key = request["public_key"]
        require(
            isinstance(public_key, str)
            and public_key.startswith("0x")
            and len(public_key) == 130,
            "public_key must be 64-byte uncompressed secp256k1 hex without the 04 prefix",
        )
        signer = int(run("cast", "keccak", public_key)[-40:], 16)
        if current:
            require(
                current == int(domain, 16)
                and self.local.values(project, "coordinatorSigner()")[0] == signer,
                "Project is bound to different coordination",
            )
            return {"bound": True, "domain": domain}
        receipt = self.local.transact(
            "owner", project, "bindCoordination(bytes32,bytes)", domain, public_key
        )
        return {"bound": True, "domain": domain, **proof(receipt)}

    def fund(self, request):
        project, amount = request["project"], int(request["amount"])
        require(0 < amount <= 2**64 - 1, "Funding must be positive u64 USDC base units")
        invoice = word(request.get("invoice_id", ZERO))
        transfer_id = request.get("transfer_id")
        recorded = transfer_id and self.local.transaction_path(transfer_id).exists()
        if (
            not recorded
            and self.local.values(
                self.token,
                "allowance(address,address)",
                self.accounts["owner"]["address"],
                project,
            )[0]
            < amount
        ):
            self.local.transact(
                "owner", self.token, "approve(address,uint256)", project, amount
            )
        receipt = self.local.transact(
            "owner",
            project,
            "coordinatedFund(uint256,bytes32)",
            amount,
            invoice,
            intent_id=transfer_id,
        )
        log = self.transaction_event(
            receipt, project, "FundingRecorded(bytes32,bytes32,bytes)"
        )
        return self.funding_receipt(log)

    def execute(self, request):
        command = request["command"]
        require(
            isinstance(command, str)
            and command.startswith("0x")
            and len(command) == 770,
            "command must contain exactly 384 bytes",
        )
        receipt = self.local.transact(
            "authority",
            request["project"],
            "executeCommand(bytes,bytes,uint8)",
            command,
            request["signature"],
            request["recovery_id"],
        )
        operation = "0x" + command[2 + 3 * 64 : 2 + 4 * 64]
        return {
            "operation_id": operation,
            "outcome": self.view({**request, "view": "outcome", "id": operation}),
            **proof(receipt),
        }

    def route_address(self, request):
        route = word(request["route_id"])
        refund = self.refund(request)
        address = self.local.values(
            request["project"],
            "predictRouteAddress(bytes32,bool)",
            route,
            str(refund).lower(),
        )[0]
        return {
            "escrow_address": f"0x{address:040x}",
            "route_id": route,
            "kind": "refund" if refund else "delivery",
        }

    @staticmethod
    def refund(request):
        kind = request.get("kind", "delivery")
        require(kind in {"delivery", "refund"}, "Route kind must be delivery or refund")
        return kind == "refund"

    def route_deposit(self, request):
        amount = int(request["amount"])
        require(
            0 < amount <= 2**64 - 1, "Transfer must be positive u64 USDC base units"
        )
        address = self.route_address(request)["escrow_address"]
        receipt = self.local.transact(
            "liquidity",
            self.token,
            "transfer(address,uint256)",
            address,
            amount,
            intent_id=request.get("transfer_id"),
        )
        return {"amount": str(amount), "escrow_address": address, **proof(receipt)}

    @staticmethod
    def event_topic(name):
        return run("cast", "keccak", name + "(bytes32,bytes32,bytes)")

    def read_funding(self, request):
        if request.get("transfer_id") is not None:
            path = self.local.transaction_path(request["transfer_id"])
            if not path.exists():
                return None
            payment = read_json(path)
            expected = {
                "role": "owner",
                "target": request["project"].lower(),
                "signature": "coordinatedFund(uint256,bytes32)",
                "args": [
                    str(int(request["amount"])),
                    word(request.get("invoice_id", ZERO)),
                ],
            }
            require(payment["binding"] == expected, "Funding intent binding mismatch")
            receipt = self.local.rpc("eth_getTransactionReceipt", [payment["tx_hash"]])
            if receipt is None:
                return None
            require(int(receipt["status"], 16) == 1, "Funding transaction reverted")
            log = self.transaction_event(
                receipt, request["project"], "FundingRecorded(bytes32,bytes32,bytes)"
            )
            self.local.wait_finalized(log)
            return self.funding_receipt(log)
        funding = word(request["funding_id"])
        logs = self.local.rpc(
            "eth_getLogs",
            [
                {
                    "address": request["project"],
                    "fromBlock": "0x0",
                    "toBlock": "latest",
                    "topics": [self.event_topic("FundingRecorded"), funding],
                }
            ],
        )
        require(len(logs) == 1, "Funding receipt not found or ambiguous")
        self.local.wait_finalized(logs[0])
        return self.funding_receipt(logs[0])

    @staticmethod
    def funding_receipt(log):
        data = bytes.fromhex(log["data"][2:])
        require(
            len(data) == 224
            and int.from_bytes(data[:32]) == 32
            and int.from_bytes(data[32:64]) == 160,
            "Malformed funding receipt",
        )
        payload = data[64:]
        words = [payload[i : i + 32] for i in range(0, 160, 32)]
        domain, funding, invoice, amount, payer = words
        require(
            "0x" + domain.hex() == log["topics"][2].lower()
            and "0x" + funding.hex() == log["topics"][1].lower(),
            "Funding identity mismatch",
        )
        return {
            "payload": "0x" + payload.hex(),
            "domain": "0x" + domain.hex(),
            "funding_id": "0x" + funding.hex(),
            "invoice_id": "0x" + invoice.hex(),
            "amount": str(int.from_bytes(amount)),
            "payer": "0x" + payer.hex(),
            "emitter": log["address"],
            "log_index": str(int(log["logIndex"], 16)),
            "topics": log["topics"],
            **proof(log),
        }

    def collect(self, request):
        receipt = self.local.transact(
            "authority",
            request["project"],
            "collectRoute(bytes32,bool)",
            word(request["route_id"]),
            str(self.refund(request)).lower(),
        )
        return {"route": self.view({**request, "view": "route"}), **proof(receipt)}

    def agent(self, request):
        project, method = request["project"], request["method"]
        task = int(request.get("task_id", 0))
        amount = int(request.get("amount", 0))
        purpose = word(
            request.get(
                "purpose_ref",
                "0x" + hashlib.sha256(b"local cross-chain task").hexdigest(),
            )
        )
        methods = {
            "create_task": ("createTask(uint256,bytes32)", [amount, purpose]),
            "request_budget": (
                "requestBudget(uint256,uint256,bytes32)",
                [task, amount, purpose],
            ),
            "withdraw": ("withdraw(uint256,uint256)", [task, amount]),
            "return_funds": ("returnFunds(uint256,uint256)", [task, amount]),
            "release": ("release(uint256,uint256)", [task, amount]),
            "finalize": (
                "finalize(uint256,bytes32)",
                [task, word(request.get("report_ref", ZERO))],
            ),
        }
        require(method in methods, "Unknown agent method")
        if method == "return_funds":
            self.local.transact(
                "agent", self.token, "approve(address,uint256)", project, amount
            )
        signature, args = methods[method]
        receipt = self.local.transact("agent", project, signature, *args)
        request_id = None
        if method == "create_task":
            log = self.transaction_event(
                receipt, project, "TaskCreated(uint256,address,uint256)"
            )
            require(
                len(log["topics"]) == 3
                and int(log["topics"][2], 16)
                == int(self.accounts["agent"]["address"], 16)
                and len(log["data"]) == 66,
                "Malformed TaskCreated event",
            )
            task = int(log["topics"][1], 16)
            request_id = str(int(log["data"], 16))
        elif method == "request_budget":
            log = self.transaction_event(
                receipt,
                project,
                "BudgetRequested(uint256,uint256,uint256,bytes32,uint8)",
            )
            require(
                len(log["topics"]) == 3
                and int(log["topics"][2], 16) == task
                and len(log["data"]) == 194
                and int(log["data"][2:66], 16) == amount
                and word("0x" + log["data"][66:130]) == purpose,
                "Malformed BudgetRequested event",
            )
            request_id = str(int(log["topics"][1], 16))
        state = self.view({"project": project, "view": "task", "id": task})
        return {
            "task_id": str(task),
            "request_id": (
                request_id if request_id is not None else state["live_request"]
            ),
            "task": state,
            **proof(receipt),
        }

    @staticmethod
    def transaction_event(receipt, project, signature):
        """Resolve IDs from this transaction, never from mutable global counters."""
        topic = run("cast", "keccak", signature).lower()
        logs = [
            log
            for log in receipt["logs"]
            if log["address"].lower() == project.lower()
            and log["topics"]
            and log["topics"][0].lower() == topic
        ]
        require(len(logs) == 1, f"Missing or ambiguous {signature} event")
        return logs[0]

    def view(self, request):
        project, kind = request["project"], request["view"]
        if kind == "project":
            result = {
                name: str(self.local.values(project, method)[0])
                for name, method in {
                    "free": "freeBalance()",
                    "held": "heldBalance()",
                    "allocated": "allocatedBalance()",
                    "task_count": "taskCount()",
                    "request_count": "requestCount()",
                }.items()
            }
            result.update(
                domain=word(self.local.values(project, "coordinationDomain()")[0]),
                balance=str(
                    self.local.values(self.token, "balanceOf(address)", project)[0]
                ),
                liquidity_balance=str(
                    self.local.values(
                        self.token,
                        "balanceOf(address)",
                        self.accounts["liquidity"]["address"],
                    )[0]
                ),
            )
            return result
        if kind == "task":
            values = self.local.values(project, "getTask(uint256)", int(request["id"]))
            return {
                "agent": f"0x{values[0]:040x}",
                "state": str(values[1] - 1),
                "paused": bool(values[2]),
                "started": bool(values[3]),
                "available": str(values[4]),
                "issued": str(values[5]),
                "recovered": str(values[6]),
                "live_request": str(values[7]),
                "report_ref": word(values[8]),
                "control_nonce": str(
                    self.local.values(
                        project, "controlNonce(uint256)", int(request["id"])
                    )[0]
                ),
            }
        if kind == "request":
            values = self.local.values(
                project, "getRequest(uint256)", int(request["id"])
            )
            return {
                "task_id": str(values[0]),
                "amount": str(values[1]),
                "purpose": word(values[2]),
                "state": str(values[3] - 1),
                "kind": str(values[4]),
            }
        if kind == "plan":
            values = self.local.values(project, "getPlan(bytes32)", word(request["id"]))
            return {"state": str(values[0]), "held": str(values[1])}
        if kind == "outcome":
            values = self.local.values(
                project, "getCommandOutcome(bytes32)", word(request["id"])
            )
            return {
                "digest": word(values[0]),
                "action": str(values[1]),
                "state": str(values[2]),
                "result": word(values[3]),
            }
        if kind == "route":
            route = word(request.get("route_id", request.get("id", ZERO)))
            values = self.local.values(
                project,
                "getRoute(bytes32,bool)",
                route,
                str(self.refund(request)).lower(),
            )
            predicted = self.route_address({**request, "route_id": route})
            address = (
                f"0x{values[1]:040x}" if values[1] else predicted["escrow_address"]
            )
            return {
                **predicted,
                "registered": values[1] != 0,
                "plan": word(values[0]),
                "escrow_address": address,
                "expected": str(values[2]),
                "received": str(values[3]),
                "dispatched": str(values[4]),
                "escrow_balance": str(
                    self.local.values(self.token, "balanceOf(address)", address)[0]
                ),
            }
        raise RuntimeError("Unknown bridge view")

    def checkpoint(self, request):
        observation = word(request["observation_id"])
        existing = self.checkpoint_log(request["project"], observation)
        if existing is None:
            self.local.transact(
                "authority",
                request["project"],
                "checkpoint(bytes32,uint256,uint256,bytes32,bytes32,bool,bytes32)",
                observation,
                int(request.get("task_id", 0)),
                int(request.get("request_id", 0)),
                word(request.get("plan_id", ZERO)),
                word(request.get("route_id", ZERO)),
                str(self.refund(request)).lower(),
                word(request.get("operation_id", ZERO)),
            )
        result = self.read_checkpoint(request)
        snapshot = result["snapshot"]
        for input_name, field in {
            "task_id": "task",
            "request_id": "request",
            "plan_id": "plan",
            "route_id": "route",
            "operation_id": "operation",
        }.items():
            expected = request.get(input_name, ZERO if field in IDENTITIES else 0)
            require(
                word(snapshot[field]) == word(expected),
                "Observation ID already bound to different selected objects",
            )
        if int(snapshot["route_kind"]) != 0:
            require(
                int(snapshot["route_kind"]) == (2 if self.refund(request) else 1),
                "Observation ID already bound to another route kind",
            )
        return result

    def checkpoint_log(self, project, observation):
        topic = run("cast", "keccak", "CheckpointRecorded(bytes32,bytes32,bytes)")
        logs = self.local.rpc(
            "eth_getLogs",
            [
                {
                    "address": project,
                    "fromBlock": "0x0",
                    "toBlock": "latest",
                    "topics": [topic, observation],
                }
            ],
        )
        require(len(logs) <= 1, "Ambiguous immutable checkpoint")
        return logs[0] if logs else None

    def read_checkpoint(self, request):
        observation = word(request["observation_id"])
        log = self.checkpoint_log(request["project"], observation)
        require(log is not None, "Checkpoint not found")
        self.local.wait_finalized(log)
        data = bytes.fromhex(log["data"][2:])
        require(
            len(data) == 64 + 1248
            and int.from_bytes(data[:32]) == 32
            and int.from_bytes(data[32:64]) == 1248,
            "Malformed checkpoint event",
        )
        payload = data[64:]
        words = [payload[i : i + 32] for i in range(0, len(payload), 32)]
        snapshot = {
            name: (
                "0x" + value.hex() if name in IDENTITIES else str(int.from_bytes(value))
            )
            for name, value in zip(CHECKPOINT_FIELDS, words, strict=True)
        }
        require(
            snapshot["observation"] == observation
            and snapshot["domain"].lower() == log["topics"][2].lower(),
            "Checkpoint identity mismatch",
        )
        require(
            int(snapshot["height"]) == int(log["blockNumber"], 16),
            "Checkpoint block mismatch",
        )
        return {
            "payload": "0x" + payload.hex(),
            "snapshot": snapshot,
            "emitter": request["project"],
            "log_index": str(int(log["logIndex"], 16)),
            "topics": log["topics"],
            **proof(log),
        }
