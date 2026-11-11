"""Relay immutable coordinator commands and reconcile native financial evidence."""

import time

from chains import PendingEvidence
from protocol import (
    ZERO,
    Action,
    CommandState,
    canonical_json,
    decode_command,
    decode_funding,
    digest,
    identity,
    normalize_signature,
    raw_id,
)


class Relayer:
    def __init__(self, network):
        self.network = network
        self.last_scan = time.monotonic()

    def tick(self):
        net = self.network
        self.reconcile_funding()
        if time.monotonic() - self.last_scan >= 30:
            self.scan_local_changes()
            self.last_scan = time.monotonic()
        self.recover_plans()
        for record in net.commands():
            self.reconcile_command(record)
        for plan_id, _metadata in net.journal.records("plan"):
            try:
                plan = net.view("get_plan", plan=raw_id(plan_id))
                if plan["status"] not in ("Funded", "Closed"):
                    net.call("advance", role="agent", plan=raw_id(plan_id))
                    plan = net.view("get_plan", plan=raw_id(plan_id))
                net.journal.put("plan_state", plan_id, plan)
                self.collect_routes(plan_id, plan)
                self.watch_plan(plan_id, plan)
                net.journal.resolve("plan_error", plan_id)
            except Exception as error:
                net.journal.put(
                    "plan_error", plan_id, {"error": str(error), "at": int(time.time())}
                )

    def reconcile_command(self, record):
        """Execute one immutable command and reconcile its native outcome."""
        net = self.network
        operation = record["operation"]
        if CommandState(record["state"]).terminal:
            net.journal.resolve("relay_error", operation)
            return
        try:
            command = decode_command(record["command"])
            attention = False
            if command["related"] != ZERO and command["action"] != Action.CLOSE_PLAN:
                plan_state = net.view("get_plan", plan=raw_id(command["related"]))
                attention = bool(plan_state and plan_state.get("requires_attention"))
            if record.get("signature") is None:
                if not attention and not record.get("signing", False):
                    net.call(
                        "request_signature",
                        role="agent",
                        wait=False,
                        operation=operation,
                    )
                return
            name, binding = net.binding(command["domain"])
            signature, recovery = normalize_signature(
                record["signature"], record["digest"], net.config["sign_key"]
            )
            previous = net.journal.get("command_evidence", operation)
            if (
                previous is None
                or not CommandState(previous["snapshot"]["command_state"]).terminal
            ):
                recipient = None
                if command["action"] == Action.OWNER_WITHDRAW:
                    recipient = binding["owner"]
                elif command["action"] == Action.DISPATCH and name == "near":
                    route = net.journal.get(
                        "route_binding", raw_id(command["reference"])
                    )
                    if route is None:
                        raise PendingEvidence(
                            "dispatch requires its committed provider quote"
                        )
                    recipient = route["quote"]["quote"]["depositAddress"]
                    if (
                        net.chains[name].recipient_identity(recipient)
                        != command["recipient"]
                    ):
                        raise ValueError(
                            "quoted dispatch recipient differs from signed command"
                        )
                # Repeating this exact authenticated command is safe on chain.
                # A timed-out transaction never generates a new operation ID.
                try:
                    if not attention:
                        net.chains[name].execute_command(
                            {**record, "signature": signature, "recovery_id": recovery},
                            recipient=recipient,
                        )
                except Exception as error:
                    net.journal.put(
                        "relay_error",
                        operation,
                        {
                            "stage": "execute",
                            "error": str(error),
                            "at": int(time.time()),
                        },
                    )
                selectors = {"operation": command["operation"]}
                if command["related"] != ZERO:
                    selectors["plan"] = command["related"]
                if command["action"] in (
                    Action.EXPECT_DELIVERY,
                    Action.EXPECT_REFUND,
                    Action.DISPATCH,
                ):
                    selectors["route"] = command["reference"]
                    selectors["kind"] = (
                        "delivery"
                        if command["action"] == Action.EXPECT_DELIVERY
                        else "refund"
                    )
                if command["action"] in (
                    Action.ALLOCATE,
                    Action.REJECT,
                    Action.OUT_OF_BUDGET,
                    Action.PAUSE,
                    Action.RECLAIM,
                    Action.CONTINUE,
                    Action.CANCEL,
                    Action.RESTORE_REQUEST,
                ):
                    selectors["task"] = command["task"]
                if command["action"] in (Action.ALLOCATE, Action.REJECT):
                    selectors["request"] = command["request"]
                evidence = net.checkpoint(name, **selectors)
                net.journal.put("command_evidence", operation, evidence)
            else:
                evidence = previous
            if evidence["snapshot"]["command_state"] == CommandState.MISSING:
                raise PendingEvidence(
                    "command execution is unconfirmed; no funds released"
                )
            submitted = net.journal.get("relay_observation", operation, {})
            if (
                submitted.get("observation") != evidence["snapshot"]["observation"]
                or time.time() - submitted.get("time", 0) > 45
            ):
                net.observe(evidence)
                net.journal.put(
                    "relay_observation",
                    operation,
                    {
                        "observation": evidence["snapshot"]["observation"],
                        "time": time.time(),
                    },
                )
            net.journal.resolve("relay_error", operation)
        except Exception as error:
            net.journal.put(
                "relay_error",
                operation,
                {"stage": "reconcile", "error": str(error), "at": int(time.time())},
            )

    def watch_plan(self, plan_id, plan):
        """Bound automatic observation retries without creating replacement sends."""
        if plan["status"] in ("Funded", "Closed") or plan.get("requires_attention"):
            return
        net = self.network
        commands = [
            {
                "operation": command["operation"],
                "state": command["state"],
                "signed": command.get("signature") is not None,
                "signing": command.get("signing", False),
            }
            for command in net.commands()
            if raw_id(command["plan"]) == raw_id(plan_id)
        ]
        fingerprint = digest(
            canonical_json(
                {
                    "status": plan["status"],
                    "routes": plan["routes"],
                    "commands": commands,
                }
            )
        ).hex()
        previous = net.journal.get("plan_watch", plan_id, {})
        if previous.get("fingerprint") != fingerprint:
            net.journal.put(
                "plan_watch",
                plan_id,
                {"fingerprint": fingerprint, "checked": time.time()},
            )
        elif time.time() - previous["checked"] >= 60:
            # The authority-backed local worker consumes one bounded observation
            # retry. The coordinator stops new progression at its configured limit.
            net.call(
                "retry",
                plan=raw_id(plan_id),
                error="No new finalized evidence within the observation interval",
                unverified=f"Plan phase {plan['status']}; existing command and transfer IDs retained",
            )
            net.journal.put(
                "plan_watch",
                plan_id,
                {"fingerprint": fingerprint, "checked": time.time()},
            )

    def reconcile_funding(self, intent_id=None):
        """Recover actual funding receipts and acknowledgements, never token sends."""
        net = self.network
        for key, intent in net.journal.records("funding_intent"):
            if intent_id is not None and key != intent_id:
                continue
            try:
                name = intent["chain"]
                effect = net.journal.effect(intent["effect_id"])
                if effect is None:
                    continue  # CLI has not yet committed to making the payment.
                if effect["request"] != intent:
                    raise ValueError(
                        "funding effect does not match its immutable intent"
                    )
                if effect["state"] == "Confirmed":
                    net.journal.resolve("funding_error", key)
                    continue
                receipt = net.journal.get("funding_native", key)
                if receipt is None:
                    receipt = net.chains[name].read_funding(intent)
                if receipt is None:
                    raise PendingEvidence(
                        "no finalized funding receipt; token transfer will not be repeated"
                    )
                snapshot = decode_funding(receipt["payload"])
                if (
                    snapshot["domain"] != intent["domain"]
                    or snapshot["invoice_id"] != intent["invoice_id"]
                    or snapshot["amount"] != int(intent["amount"])
                    or snapshot["payer"] != net.config["chains"][name]["owner_identity"]
                ):
                    raise ValueError(
                        "native funding receipt differs from the committed intent"
                    )
                if name == "solana" and snapshot["funding_id"] != intent["native_id"]:
                    raise ValueError(
                        "native funding receipt identity differs from the intent"
                    )
                if (
                    name == "near"
                    and identity(receipt["funding_ref"]) != intent["native_id"]
                ):
                    raise ValueError("native funding reference differs from the intent")
                evidence = {
                    "kind": "funding",
                    "chain": name,
                    "payload": receipt["payload"],
                    "proof": receipt,
                    "snapshot": snapshot,
                }
                net.verify_evidence(evidence)
                previous = net.journal.get("funding_native", key)
                if previous is not None and previous["payload"] != receipt["payload"]:
                    raise ValueError("immutable funding receipt changed")
                net.journal.put(
                    "funding_native", key, previous or receipt, immutable=True
                )
                receipt_key = f"{name}:{snapshot['funding_id']}"
                existing = net.journal.get("funding", receipt_key)
                if existing is not None and existing["payload"] != evidence["payload"]:
                    raise ValueError("immutable funding receipt changed")
                net.journal.put(
                    "funding", receipt_key, existing or evidence, immutable=True
                )
                net.journal.settle_effect(intent["effect_id"], receipt)
                net.journal.resolve("funding_error", key)
            except Exception as error:
                net.journal.note_unknown(intent["effect_id"], error)
                net.journal.put(
                    "funding_error", key, {"error": str(error), "at": int(time.time())}
                )
        # Coordinator acknowledgement is separate from confirmed native funding.
        for key, evidence in net.journal.records("funding"):
            if (
                net.journal.get("funding_observation", key, {}).get("state")
                == "Confirmed"
            ):
                net.journal.resolve("funding_error", key)
                continue
            try:
                result = net.observe_funding(evidence["chain"], evidence["proof"])
                if result["state"] == "Confirmed":
                    net.journal.resolve("funding_error", key)
            except Exception as error:
                net.journal.put(
                    "funding_error", key, {"error": str(error), "at": int(time.time())}
                )

    def recover_plans(self):
        """Discover committed plans even when Apply's response was lost locally."""
        net = self.network
        for counter in range(1, int(net.view("get_state")["next_plan"])):
            plan_id = raw_id(identity(counter))
            try:
                self.recover_plan(plan_id)
                net.journal.resolve("plan_recovery_error", plan_id)
            except Exception as error:
                net.journal.put(
                    "plan_recovery_error",
                    plan_id,
                    {"error": str(error), "stage": "recovery", "at": int(time.time())},
                )

    def scan_local_changes(self):
        """Discover native agent activity without requiring the convenience CLI."""
        net = self.network
        for name, binding in net.config["chains"].items():
            try:
                cursor = int(net.journal.get("scan_cursor", name, 0))
                local = net.chains[name].scan(cursor)
                tasks = local["tasks"]
                cursor = (
                    int(local["next_task"])
                    if int(local["next_task"]) < int(local["total"])
                    else 0
                )
                net.journal.put("scan_cursor", name, cursor)
                for task in tasks:
                    task_id = identity(task["task"])
                    key = f"{name}:{task_id}"
                    fingerprint = digest(canonical_json(task)).hex()
                    old = net.journal.get("local_observation", key, {})
                    if old.get("fingerprint") != fingerprint:
                        evidence = net.checkpoint(
                            name, task=task_id, request=task.get("request")
                        )
                        old = {
                            "fingerprint": fingerprint,
                            "evidence": evidence,
                            "submitted": 0,
                        }
                        net.journal.put("local_observation", key, old)
                    actual = net.view(
                        "get_task",
                        domain=raw_id(binding["domain"]),
                        task=raw_id(task_id),
                    )
                    if (
                        actual is not None
                        and int(actual["sequence"])
                        >= old["evidence"]["snapshot"]["sequence"]
                    ):
                        continue
                    if time.time() - old["submitted"] >= 45:
                        net.observe(old["evidence"])
                        old["submitted"] = time.time()
                        net.journal.put("local_observation", key, old)
                known = net.view("get_domain", domain=raw_id(binding["domain"]))
                if int(local["free"]) != int(known["free"]) or int(
                    local["held"]
                ) != int(known["held"]):
                    net.observe(net.checkpoint(name))
                net.journal.resolve("scan_error", name)
            except Exception as error:
                net.journal.put(
                    "scan_error", name, {"error": str(error), "at": int(time.time())}
                )

    def recover_plan(self, plan_id):
        net = self.network
        plan = net.view("get_plan", plan=plan_id)
        if plan is None:
            raise PendingEvidence(
                "committed plan is not visible in finalized coordinator state"
            )
        destination, _ = net.binding(plan["destination"])
        if net.journal.get("plan", plan_id) is None:
            net.journal.put(
                "plan",
                plan_id,
                {
                    "destination": destination,
                    "routes": [route["quote"] for route in plan["routes"]],
                },
                immutable=True,
            )
        for route in plan["routes"]:
            quote = route["quote"]
            route_id = raw_id(quote["route"])
            if net.journal.get("route_binding", route_id) is not None:
                continue
            response = net.journal.get("route_quote", route_id)
            if response is None:
                raise PendingEvidence("committed route has no persisted provider quote")
            if digest(canonical_json(response)).hex() != raw_id(quote["quote_hash"]):
                raise ValueError("persisted quote differs from coordinator commitment")
            source, _ = net.binding(quote["source"])
            net.journal.put(
                "route_binding",
                route_id,
                {
                    "source": source,
                    "destination": destination,
                    "quote_id": route_id,
                    "quote": response,
                    "plan": plan_id,
                },
                immutable=True,
            )

    def collect_routes(self, plan_id, plan):
        net = self.network
        destination, _ = net.binding(plan["destination"])
        for route in plan["routes"]:
            quote = route["quote"]
            source, _ = net.binding(quote["source"])
            for name, kind, expected in (
                (destination, "delivery", quote["output"]),
                (source, "refund", quote["input"]),
            ):
                try:
                    state = net.chains[name].route_view(quote["route"], kind)
                    exists = state["exists"]
                    if not isinstance(exists, bool):
                        raise ValueError("route existence must be a boolean")
                    if not exists:
                        # ExpectDelivery/ExpectRefund may not have finalized yet.
                        net.journal.resolve(
                            "collection_error", f"{name}:{quote['route']}"
                        )
                        continue
                    balance = int(state["escrow_balance"])
                    received = int(state["received"])
                    known = int(
                        route["received"] if kind == "delivery" else route["returned"]
                    )
                    collectable = (
                        received < int(expected)
                        and balance > 0
                        and (kind == "refund" or balance >= int(expected))
                    )
                    if collectable:
                        net.chains[name].collect_route(quote["route"], kind)
                    if collectable or received > known:
                        evidence = net.checkpoint(
                            name,
                            plan=identity(plan_id),
                            route=identity(quote["route"]),
                            kind=kind,
                        )
                        net.journal.put(
                            "route_evidence", f"{name}:{quote['route']}", evidence
                        )
                        net.observe(evidence)
                    net.journal.resolve("collection_error", f"{name}:{quote['route']}")
                except Exception as error:
                    net.journal.put(
                        "collection_error",
                        f"{name}:{quote['route']}",
                        {"error": str(error), "at": int(time.time())},
                    )
