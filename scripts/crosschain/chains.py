"""Local RPC adapters and independently fetched, finalized provider evidence."""

import base64
import json
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from localnet import ROOT, Environment, LocalnetError  # noqa: E402
from protocol import (  # noqa: E402
    ZERO,
    base58_decode,
    base58_encode,
    decode_checkpoint,
    decode_funding,
    digest,
    hex_bytes,
    identity,
    raw_id,
    unhex,
)


class PendingEvidence(RuntimeError):
    """No affirmative finalized evidence yet; this never authorizes a resend."""


def http_json(url, body=None, timeout=15):
    destination = urlsplit(url)
    if (
        destination.scheme != "http"
        or destination.hostname != "127.0.0.1"
        or destination.port is None
        or destination.username is not None
        or destination.password is not None
    ):
        raise ValueError("local services accept only explicit IPv4 loopback URLs")

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *_args, **_kwargs):
            raise ValueError(
                "local services must not redirect RPC or provider requests"
            )

    request = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=timeout) as response:
        if int(response.headers.get("Content-Length", "0")) > 8_000_000:
            raise ValueError("oversized local response")
        raw = response.read(8_000_001)
        if len(raw) > 8_000_000:
            raise ValueError("oversized local response")
        return json.loads(raw)


class Chain:
    def __init__(self, name, root=ROOT, binding=None):
        self.name = name
        self.binding = binding
        self.environment = Environment(name, root=root)

    def rpc(self, method, params):
        result = http_json(
            self.environment.url(),
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            },
        )
        if "error" in result:
            raise PendingEvidence(f"{self.name} {method}: {result['error']}")
        return result["result"]

    def bridge(self, action, **arguments):
        result = subprocess.run(
            [str(self.environment.adapter), "bridge"],
            input=json.dumps({"action": action, **arguments}),
            text=True,
            capture_output=True,
            cwd=self.environment.root,
            env=self.environment.environment(),
            timeout=240,
            check=False,
        )
        if result.returncode:
            # No retry here: the transaction might already exist on its chain.
            raise LocalnetError(
                f"{self.name} {action}: {result.stderr[-6000:] or result.stdout[-2000:]}"
            )
        try:
            return json.loads(result.stdout)
        except ValueError as error:
            raise LocalnetError(
                f"{self.name} {action}: adapter returned invalid JSON"
            ) from error

    def checkpoint(self, observation, **selectors):
        kind = selectors.pop("kind", None)
        arguments = {
            key: identity(value)
            for key, value in selectors.items()
            if value is not None
        }
        if self.name == "ethereum":
            arguments = {f"{key}_id": value for key, value in arguments.items()}
            for key in ("task_id", "request_id"):
                if key in arguments:
                    arguments[key] = str(int(arguments[key], 16))
            if kind is not None:
                arguments["kind"] = kind
            return self.bridge(
                "checkpoint",
                project=self.binding["vault"],
                observation_id=identity(observation),
                **arguments,
            )
        if self.name == "near":
            for key in ("task", "request"):
                if key in arguments:
                    arguments[key] = int(arguments[key], 16)
        return self.bridge("checkpoint", observation=identity(observation), **arguments)

    def evidence_request(self, receipt):
        """Translate the native immutable receipt envelope into an MPC request."""
        if self.name == "ethereum":
            return {
                "Ethereum": {
                    "tx_id": raw_id(receipt["tx_hash"]),
                    "finality": "Finalized",
                    "extractors": [{"Log": {"log_index": int(receipt["log_index"])}}],
                }
            }
        if self.name == "solana":
            proof = receipt["proof"]
            return {
                "Solana": {
                    "tx_id": base58_decode(proof["tx_hash"], 64).hex(),
                    "finality": "Finalized",
                    "extractors": [
                        {
                            "AccountState": {
                                "pubkey": base58_decode(
                                    proof["receipt_account"], 32
                                ).hex(),
                            }
                        }
                    ],
                }
            }
        raise ValueError("native NEAR receipt does not require foreign evidence")

    def route_address(self, route, kind):
        if self.name == "ethereum":
            return self.bridge(
                "route_address",
                project=self.binding["vault"],
                route_id=identity(route),
                kind=kind,
            )["escrow_address"]
        result = self.bridge("route_address", route=identity(route), kind=kind)
        return result["receiver"] if self.name == "solana" else result

    def route_view(self, route, kind):
        if self.name == "ethereum":
            record = self.bridge(
                "view",
                project=self.binding["vault"],
                view="route",
                id=identity(route),
                kind=kind,
            )
            return {**record, "exists": record["registered"]}
        if self.name == "solana":
            return self.bridge("view", route=identity(route), kind=kind)["route"]
        record = self.near_view(
            self.binding["vault"], "get_route", {"route": raw_id(identity(route))}
        )
        if record is None:
            return {"exists": False}
        balance = self.near_view(
            self.binding["asset"], "ft_balance_of", {"account_id": record["receiver"]}
        )
        return {**record, "exists": True, "escrow_balance": balance}

    def collect_route(self, route, kind):
        if self.name == "ethereum":
            return self.bridge(
                "collect",
                project=self.binding["vault"],
                route_id=identity(route),
                kind=kind,
            )
        return self.bridge("collect", route=identity(route), kind=kind)

    def execute_command(self, record, recipient=None):
        """Submit the exact command with its already verified normalized signature."""
        arguments = {
            "command": hex_bytes(unhex(record["command"])),
            "signature": record["signature"],
            "recovery_id": record["recovery_id"],
        }
        if self.name == "ethereum":
            arguments["project"] = self.binding["vault"]
        elif self.name == "near":
            arguments["recipient_id"] = recipient
        return self.bridge("execute", **arguments)

    def create_task(self, amount, purpose):
        if self.name == "ethereum":
            native = self.bridge(
                "agent",
                project=self.binding["vault"],
                method="create_task",
                amount=str(amount),
                purpose_ref=purpose,
            )
            task, request = (
                identity(int(native["task_id"])),
                identity(int(native["request_id"])),
            )
        elif self.name == "solana":
            native = self.bridge(
                "agent", method="create", amount=str(amount), purpose=purpose
            )
            task, request = identity(native["task"]), identity(native["request"])
        else:
            native = self.bridge(
                "agent",
                method="create_task",
                args={
                    "amount": str(amount),
                    "purpose": list(unhex(purpose, 32)),
                },
            )
            task = identity(int(native))
            # Execution can precede the finalized view. Re-read the returned ID;
            # retrying creation would make a second task and financial request.
            deadline = time.monotonic() + 60
            while True:
                state = self.near_view(
                    self.binding["vault"], "get_task", {"task_id": int(task, 16)}
                )
                if state is not None:
                    break
                if time.monotonic() >= deadline:
                    raise PendingEvidence(
                        f"NEAR task {task} was created; its finalized view is pending"
                    )
                time.sleep(0.5)
            request = identity(int(state["live_request"]))
        return {"task": task, "request": request, "native": native}

    def agent_action(self, task, action, amount=None):
        task = identity(task)
        purpose = digest(b"additional local funding")
        if self.name == "ethereum":
            arguments = {
                "project": self.binding["vault"],
                "method": {"request": "request_budget", "return": "return_funds"}.get(
                    action, action
                ),
                "task_id": str(int(task, 16)),
                "purpose_ref": hex_bytes(purpose),
                "report_ref": ZERO,
            }
        elif self.name == "solana":
            arguments = {
                "method": action,
                "task": task,
                "purpose": hex_bytes(purpose),
                "report": ZERO,
            }
        elif action == "return":
            return self.bridge(
                "call",
                role="agent",
                contract=self.binding["asset"],
                method="ft_transfer_call",
                deposit="1",
                args={
                    "receiver_id": self.binding["vault"],
                    "amount": str(amount),
                    "msg": json.dumps({"kind": "return", "task_id": int(task, 16)}),
                },
            )
        else:
            arguments = {"task_id": int(task, 16)}
            if amount is not None:
                arguments["amount"] = str(amount)
            if action == "request":
                arguments["purpose"] = list(purpose)
            if action == "finalize":
                arguments["report"] = None
            method = "request_budget" if action == "request" else action
            return self.bridge("agent", method=method, args=arguments)
        if amount is not None:
            arguments["amount"] = str(amount)
        return self.bridge("agent", **arguments)

    def fund(self, intent):
        arguments = {"amount": intent["amount"], "invoice_id": intent["invoice_id"]}
        if self.name == "ethereum":
            arguments.update(
                project=self.binding["vault"], transfer_id=intent["effect_id"]
            )
        elif self.name == "solana":
            arguments["funding_id"] = intent["native_id"]
        else:
            arguments["funding_ref"] = intent["native_id"]
        return self.bridge("fund", **arguments)

    def read_funding(self, intent):
        if self.name == "ethereum":
            arguments = {
                "project": self.binding["vault"],
                "transfer_id": intent["effect_id"],
                "amount": intent["amount"],
                "invoice_id": intent["invoice_id"],
            }
        elif self.name == "solana":
            arguments = {"funding_id": intent["native_id"]}
        else:
            arguments = {"funding_ref": intent["native_id"]}
        return self.bridge("read_funding", **arguments)

    def deposit_route(self, route, kind, amount, effect_id):
        if self.name == "ethereum":
            return self.bridge(
                "route_deposit",
                project=self.binding["vault"],
                route_id=identity(route),
                kind=kind,
                amount=str(amount),
                transfer_id=effect_id,
            )
        return self.bridge(
            "route_deposit", route=identity(route), kind=kind, amount=str(amount)
        )

    def scan(self, cursor, limit=10):
        """Read a page of local tasks; IDs are common words, never truthy sentinels."""
        if self.name == "solana":
            page = self.bridge("scan", **{"from": cursor, "limit": limit})
            return {
                **page,
                "tasks": [
                    {
                        **task,
                        "task": identity(task["task"]),
                        "request": identity(task["request"])
                        if task["request"] is not None
                        else None,
                    }
                    for task in page["tasks"]
                ],
            }
        tasks = []
        if self.name == "ethereum":
            local = self.bridge("view", project=self.binding["vault"], view="project")
            total = int(local["task_count"])
            for task_id in range(cursor + 1, min(cursor + limit, total) + 1):
                value = self.bridge(
                    "view", project=self.binding["vault"], view="task", id=str(task_id)
                )
                live = int(value["live_request"])
                request = (
                    self.bridge(
                        "view",
                        project=self.binding["vault"],
                        view="request",
                        id=str(live),
                    )
                    if live
                    else None
                )
                tasks.append(
                    {
                        "task": identity(task_id),
                        "request": identity(live) if live else None,
                        "value": value,
                        "request_value": request,
                    }
                )
        else:
            local = {
                **self.near_view(self.binding["vault"], "get_project"),
                **self.near_view(self.binding["vault"], "get_bridge"),
            }
            total = int(local["next_task"])
            for task_id in range(cursor, min(cursor + limit, total)):
                value = self.near_view(
                    self.binding["vault"], "get_task", {"task_id": task_id}
                )
                live = value["live_request"]
                request = (
                    self.near_view(
                        self.binding["vault"], "get_request", {"request_id": live}
                    )
                    if live is not None
                    else None
                )
                tasks.append(
                    {
                        "task": identity(task_id),
                        "request": identity(live) if live is not None else None,
                        "value": value,
                        "request_value": request,
                    }
                )
        return {
            **local,
            "tasks": tasks,
            "next_task": str(min(cursor + limit, total)),
            "total": str(total),
        }

    def recipient_identity(self, address):
        if self.name == "ethereum":
            return identity(int.from_bytes(unhex(address, 20), "big"))
        if self.name == "solana":
            return hex_bytes(base58_decode(address, 32))
        return hex_bytes(digest(address.encode()))

    def near_view(self, contract, method, arguments=None):
        if self.name != "near":
            raise ValueError("native NEAR view used on another chain")
        result = self.rpc(
            "query",
            {
                "request_type": "call_function",
                "finality": "final",
                "account_id": contract,
                "method_name": method,
                "args_base64": base64.b64encode(
                    json.dumps(arguments or {}).encode()
                ).decode(),
            },
        )
        if "error" in result:
            raise PendingEvidence(f"NEAR {method}: {result['error']}")
        return json.loads(bytes(result["result"]))

    def evm_evidence(self, request, binding):
        args = request["Ethereum"]
        if args["finality"] != "Finalized":
            raise ValueError("unfinalized EVM evidence request")
        tx_hash = hex_bytes(unhex(args["tx_id"], 32))
        receipt = self.rpc("eth_getTransactionReceipt", [tx_hash])
        if receipt is None:
            raise PendingEvidence("EVM transaction has no receipt")
        if int(receipt["status"], 16) != 1:
            raise ValueError("failed EVM transaction is not an application receipt")
        height = int(receipt["blockNumber"], 16)
        finalized = self.rpc("eth_getBlockByNumber", ["finalized", False])
        if finalized is None or int(finalized["number"], 16) < height:
            raise PendingEvidence("EVM receipt block is not finalized")
        block = self.rpc("eth_getBlockByNumber", [receipt["blockNumber"], False])
        if block["hash"].lower() != receipt["blockHash"].lower():
            raise ValueError("EVM receipt is not canonical")
        values = []
        for extractor in args["extractors"]:
            index = extractor["Log"]["log_index"]
            matches = [
                log for log in receipt["logs"] if int(log["logIndex"], 16) == index
            ]
            if len(matches) != 1:
                raise ValueError("missing exact EVM log")
            log = matches[0]
            if log.get("removed") or log["address"].lower() != binding["vault"].lower():
                raise ValueError("wrong EVM log provenance")
            if (
                log["transactionHash"].lower() != tx_hash.lower()
                or log["blockHash"] != block["hash"]
            ):
                raise ValueError("inconsistent EVM log identity")
            data = unhex(log["data"])
            length = int.from_bytes(data[32:64], "big")
            if (
                length not in (160, 1248)
                or len(data) != length + 64
                or int.from_bytes(data[:32], "big") != 32
            ):
                raise ValueError("not a canonical application receipt payload")
            funding = length == 160
            snapshot = (
                decode_funding(data[64:]) if funding else decode_checkpoint(data[64:])
            )
            topics = log["topics"]
            expected_topic = binding[
                "funding_topic" if funding else "checkpoint_topic"
            ].lower()
            if len(topics) != 3 or topics[0].lower() != expected_topic:
                raise ValueError("wrong checkpoint event")
            receipt_id = snapshot["funding_id"] if funding else snapshot["observation"]
            if (
                topics[1].lower() != receipt_id
                or topics[2].lower() != binding["domain"].lower()
            ):
                raise ValueError("checkpoint event topic mismatch")
            if snapshot["domain"] != binding["domain"].lower():
                raise ValueError("checkpoint domain/height mismatch")
            if not funding and (
                snapshot["height"] != height
                or snapshot["timestamp"] != int(block["timestamp"], 16)
            ):
                raise ValueError("checkpoint timestamp mismatch")
            if funding and snapshot["payer"] != binding["owner_identity"].lower():
                raise ValueError("wrong funding payer")
            values.append(
                {
                    "EvmExtractedValue": {
                        "Log": {
                            "removed": False,
                            "log_index": index,
                            "transaction_index": int(log["transactionIndex"], 16),
                            "transaction_hash": tx_hash.removeprefix("0x"),
                            "block_hash": block["hash"].removeprefix("0x"),
                            "block_number": height,
                            "address": log["address"].removeprefix("0x"),
                            "data": log["data"],
                            "topics": [topic.removeprefix("0x") for topic in topics],
                        }
                    }
                }
            )
        return values

    def solana_evidence(self, request, binding):
        args = request["Solana"]
        if args["finality"] != "Finalized":
            raise ValueError("unfinalized Solana evidence request")
        signature = base58_encode(unhex(args["tx_id"], 64))
        status = self.rpc(
            "getSignatureStatuses", [[signature], {"searchTransactionHistory": True}]
        )["value"][0]
        if status is None or status.get("confirmationStatus") != "finalized":
            raise PendingEvidence("Solana signature is not finalized")
        if status["err"] is not None:
            raise ValueError("failed Solana transaction is not evidence")
        values = []
        for extractor in args["extractors"]:
            receipt_pubkey = unhex(extractor["AccountState"]["pubkey"], 32)
            address = base58_encode(receipt_pubkey)
            result = self.rpc(
                "getAccountInfo",
                [
                    address,
                    {
                        "encoding": "base64",
                        "commitment": "finalized",
                        "minContextSlot": status["slot"],
                    },
                ],
            )
            account = result["value"]
            if account is None:
                raise PendingEvidence("immutable Solana receipt does not exist")
            if account["owner"] != binding["program"] or account["executable"]:
                raise ValueError("wrong Solana receipt owner")
            data = base64.b64decode(account["data"][0], validate=True)
            funding = len(data) == 168
            discriminator = (
                b"account:FundingReceipt" if funding else b"account:Checkpoint"
            )
            if len(data) not in (168, 1256) or data[:8] != digest(discriminator)[:8]:
                raise ValueError("wrong Solana checkpoint discriminator or length")
            snapshot = (
                decode_funding(data[8:]) if funding else decode_checkpoint(data[8:])
            )
            if snapshot["domain"] != binding["domain"].lower():
                raise ValueError("wrong Solana checkpoint domain")
            # A domain is owner-supplied at binding and can be copied into another
            # Project. The receipt address must therefore bind the registered
            # native Project too, rather than trusting program owner/domain alone.
            receipt_id = snapshot["funding_id" if funding else "observation"]
            seeds = (
                (b"funding" if funding else b"checkpoint")
                + base58_decode(binding["vault"], 32)
                + unhex(receipt_id, 32)
            )
            suffix = base58_decode(binding["program"], 32) + b"ProgramDerivedAddress"
            # No Edwards-curve implementation is needed here: the genuine source
            # program creates only canonical PDAs and cannot write other accounts.
            if not any(
                digest(seeds + bytes([bump]) + suffix) == receipt_pubkey
                for bump in range(256)
            ):
                raise ValueError("Solana receipt does not belong to registered vault")
            if not funding and snapshot["height"] > result["context"]["slot"]:
                raise ValueError("checkpoint slot is not finalized")
            if funding and snapshot["payer"] != binding["owner_identity"].lower():
                raise ValueError("wrong funding payer")
            values.append(
                {
                    "SvmExtractedValue": {
                        "AccountState": {
                            "owner": base58_decode(account["owner"], 32).hex(),
                            "data": data.hex(),
                        }
                    }
                }
            )
        return values

    def foreign_evidence(self, request, binding):
        if self.name == "ethereum":
            return self.evm_evidence(request, binding)
        if self.name == "solana":
            return self.solana_evidence(request, binding)
        raise ValueError("NEAR uses native authenticated callbacks")


class Network:
    def __init__(self, configuration, journal):
        self.config = configuration
        self.journal = journal
        self.chains = {
            name: Chain(name, binding=binding)
            for name, binding in configuration["chains"].items()
        }
        self.near = self.chains["near"]
        self.coordinator = configuration["coordinator"]
        self.mpc = configuration["mpc"]

    def view(self, method, **args):
        return self.near.near_view(self.coordinator, method, args)

    def call(
        self,
        method,
        role="authority",
        wait=True,
        deposit="1000000000000000000000000",
        **args,
    ):
        return self.near.bridge(
            "call",
            role=role,
            contract=self.coordinator,
            method=method,
            args=args,
            wait=wait,
            deposit=deposit,
        )

    def commands(self):
        result, start = [], 1
        while True:
            page = self.view("list_commands", **{"from": start, "limit": 100})
            if not isinstance(page, list):
                raise ValueError("invalid coordinator command page")
            result.extend(page)
            if len(page) < 100:
                return result
            start += len(page)

    def binding(self, domain):
        for name, binding in self.config["chains"].items():
            if raw_id(binding["domain"]) == raw_id(domain):
                return name, binding
        raise ValueError("unregistered destination domain")

    def checkpoint(self, name, **selectors):
        binding = self.config["chains"][name]
        observation = hex_bytes(digest(f"{name}:{time.time_ns()}".encode()))
        result = self.chains[name].checkpoint(observation, **selectors)
        payload = result["payload"]
        snapshot = decode_checkpoint(payload)
        if snapshot["observation"] != observation or raw_id(
            snapshot["domain"]
        ) != raw_id(binding["domain"]):
            raise ValueError("adapter checkpoint identity mismatch")
        evidence = {
            "chain": name,
            "payload": payload,
            "proof": result,
            "snapshot": snapshot,
        }
        self.journal.put("checkpoint", observation, evidence, immutable=True)
        deadline = time.monotonic() + 90
        while True:
            try:
                self.verify_evidence(evidence)
                break
            except PendingEvidence:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.5)
        return evidence

    def verify_evidence(self, evidence):
        name = evidence["chain"]
        funding = evidence.get("kind") == "funding"
        snapshot = (
            decode_funding(evidence["payload"])
            if funding
            else decode_checkpoint(evidence["payload"])
        )
        _, binding = self.binding(snapshot["domain"])
        if name == "near":
            method = "get_funding" if funding else "get_checkpoint"
            key = "funding_id" if funding else "observation"
            actual = self.near.near_view(
                binding["vault"], method, {key: raw_id(snapshot[key])}
            )
            if actual is None:
                raise PendingEvidence("NEAR checkpoint has not finalized")
            if unhex(actual) != unhex(evidence["payload"]):
                raise ValueError("NEAR checkpoint payload mismatch")
            return None, None
        request = self.chains[name].evidence_request(evidence["proof"])
        values = self.chains[name].foreign_evidence(request, binding)
        if name == "ethereum":
            actual = unhex(values[0]["EvmExtractedValue"]["Log"]["data"])[64:]
        else:
            actual = unhex(values[0]["SvmExtractedValue"]["AccountState"]["data"])[8:]
        if actual != unhex(evidence["payload"]):
            raise ValueError("foreign checkpoint bytes differ from supplied payload")
        return request, values

    def observe(self, evidence):
        snapshot = evidence["snapshot"]
        request, values = self.verify_evidence(evidence)
        if evidence["chain"] == "near":
            result = self.call(
                "observe_near",
                role="agent",
                domain=raw_id(snapshot["domain"]),
                observation=raw_id(snapshot["observation"]),
            )
        else:
            result = self.call(
                "observe_foreign",
                role="agent",
                wait=False,
                domain=raw_id(snapshot["domain"]),
                payload=raw_id(evidence["payload"]),
                request=request,
                values=values,
            )
        self.journal.put("observation_submission", snapshot["observation"], result)
        return result

    def observe_funding(self, name, receipt):
        snapshot = decode_funding(receipt["payload"])
        binding = self.config["chains"][name]
        if snapshot["domain"] != identity(binding["domain"]) or snapshot[
            "payer"
        ] != identity(binding["owner_identity"]):
            raise ValueError(
                "funding receipt does not belong to this Project owner/domain"
            )
        evidence = {
            "kind": "funding",
            "chain": name,
            "payload": receipt["payload"],
            "proof": receipt,
            "snapshot": snapshot,
        }
        request, values = self.verify_evidence(evidence)
        key = f"{name}:{snapshot['funding_id']}"
        existing = self.journal.get("funding", key)
        if existing is not None and existing["payload"] != evidence["payload"]:
            raise ValueError("immutable funding receipt changed")
        self.journal.put("funding", key, existing or evidence, immutable=True)
        if (
            self.view(
                "get_funding_seen",
                domain=raw_id(snapshot["domain"]),
                funding_id=raw_id(snapshot["funding_id"]),
            )
            is True
        ):
            status = {"state": "Confirmed", "funding_id": snapshot["funding_id"]}
            self.journal.put("funding_observation", key, status)
            return status
        previous = self.journal.get("funding_observation", key, {})
        if time.time() - previous.get("submitted_at", 0) < 45:
            return previous
        # Observation retries cannot move tokens; the coordinator deduplicates
        # immutable receipts. Persist before submission to cover a lost response.
        status = {
            "state": "Pending",
            "funding_id": snapshot["funding_id"],
            "submitted_at": time.time(),
        }
        self.journal.put("funding_observation", key, status)
        if name == "near":
            result = self.call(
                "observe_funding_near",
                role="agent",
                domain=raw_id(snapshot["domain"]),
                funding_id=raw_id(snapshot["funding_id"]),
            )
        else:
            result = self.call(
                "observe_funding_foreign",
                role="agent",
                wait=False,
                domain=raw_id(snapshot["domain"]),
                payload=raw_id(evidence["payload"]),
                request=request,
                values=values,
            )
        status["submission"] = result
        self.journal.put("funding_observation", key, status)
        return status
