"""User-initiated funding, task and route workflows over the chain adapters."""

import os
import secrets
import sys
import time

from chains import PendingEvidence
from protocol import ZERO, canonical_json, digest, hex_bytes, identity, raw_id
from providers import LiquidityClient, iso_time
from worker import Relayer

UNIT = 1_000_000


def synchronize(net, name, **selectors):
    evidence = net.checkpoint(name, **selectors)
    net.observe(evidence)
    return evidence


def wait_observation(net, name, sequence, timeout=120):
    deadline = time.monotonic() + timeout
    retry_at = time.monotonic() + 45
    binding = net.config["chains"][name]
    while time.monotonic() < deadline:
        observed = net.view("get_domain", domain=raw_id(binding["domain"]))
        if int(observed["sequence"]) >= sequence:
            return observed
        if time.monotonic() >= retry_at:
            # A submitted observation may be lost or its asynchronous provider
            # request may time out. Reuse the immutable native receipt; this
            # cannot create another payment or change the source checkpoint.
            evidence = next(
                (
                    item
                    for _, item in net.journal.records("checkpoint")
                    if item["chain"] == name
                    and item["snapshot"]["sequence"] == sequence
                ),
                None,
            )
            if evidence is not None:
                net.observe(evidence)
            retry_at = time.monotonic() + 45
        time.sleep(0.5)
    raise PendingEvidence(f"{name} observation not confirmed; inspect provider status")


def apply_request(net, name, request_id, slippage=20, source=None):
    provider = LiquidityClient(net.config["provider_url"])
    threshold = int(os.environ.get("APODECT_FEE_WARNING_BPS", "500"))
    if not 0 <= threshold <= 10000:
        raise ValueError("APODECT_FEE_WARNING_BPS must be between 0 and 10000")
    binding = net.config["chains"][name]
    request = net.view(
        "get_request",
        domain=raw_id(binding["domain"]),
        request=raw_id(identity(request_id)),
    )
    if request is None:
        raise ValueError("request must first be observed and approved")
    for chain in net.chains:
        evidence = synchronize(net, chain)
        wait_observation(net, chain, evidence["snapshot"]["sequence"])
    free = {
        chain: int(net.view("get_domain", domain=raw_id(data["domain"]))["available"])
        for chain, data in net.config["chains"].items()
    }
    local = min(int(request["amount"]), free[name])
    remainder = int(request["amount"]) - local
    routes, metadata = [], []
    expires = int(time.time()) + 1800
    sources = (
        [source]
        if source is not None
        else sorted(
            (chain for chain in net.chains if chain != name),
            key=lambda chain: free[chain],
            reverse=True,
        )
    )
    if any(chain not in net.chains or chain == name for chain in sources):
        raise ValueError("delivery source must be another registered chain")
    for source in sources:
        if remainder == 0:
            break
        # The mock quote model is deterministic. Binary search affordable output
        # using dry quotes rather than assuming principal alone covers the input.
        source_binding = net.config["chains"][source]
        route = secrets.token_hex(32)
        recipient = net.chains[name].route_address(route, "delivery")
        refund = net.chains[source].route_address(route, "refund")
        proposal = {
            "dry": True,
            "swapType": "EXACT_OUTPUT",
            "slippageTolerance": slippage,
            "originAsset": source_binding["asset_id"],
            "destinationAsset": binding["asset_id"],
            "depositType": "ORIGIN_CHAIN",
            "recipientType": "DESTINATION_CHAIN",
            "refundType": "ORIGIN_CHAIN",
            "recipient": recipient,
            "refundTo": refund,
            "deadline": iso_time(expires),
        }
        lower, upper = 0, min(remainder, free[source])
        while lower < upper:
            middle = (lower + upper + 1) // 2
            quoted = provider.quote({**proposal, "amount": str(middle)})
            if int(quoted["quote"]["amountIn"]) <= free[source]:
                lower = middle
            else:
                upper = middle - 1
        if lower == 0:
            continue
        quote = provider.quote(
            {
                **proposal,
                "dry": False,
                "amount": str(lower),
                "correlationId": route,
            },
        )
        # Keep the public response before Apply: a lost transaction response must
        # remain recoverable without reading the liquidity provider's database.
        net.journal.put("route_quote", route, quote, immutable=True)
        details = quote["quote"]
        routes.append(
            {
                "route": route,
                "source": raw_id(source_binding["domain"]),
                "input": int(details["amountIn"]),
                "min_input": int(details["minAmountIn"]),
                "output": lower,
                "deposit": raw_id(
                    net.chains[source].recipient_identity(details["depositAddress"])
                ),
                "refund": refund,
                "recipient": recipient,
                "quote_hash": digest(canonical_json(quote)).hex(),
                "expires": expires,
            }
        )
        metadata.append(
            (
                route,
                {
                    "source": source,
                    "destination": name,
                    "quote_id": route,
                    "quote": quote,
                },
            )
        )
        remainder -= lower
    if remainder:
        return {
            "state": "Waiting",
            "reason": "principal plus quoted costs exceed safely free funds",
            "shortfall": str(remainder),
        }
    plan = net.call(
        "apply",
        domain=raw_id(binding["domain"]),
        request=raw_id(identity(request_id)),
        local_amount=local,
        routes=routes,
        expires=expires,
    )
    if not isinstance(plan, str):
        raise ValueError("coordinator did not return a plan ID")
    net.journal.put(
        "plan", plan, {"destination": name, "routes": routes}, immutable=True
    )
    for route, value in metadata:
        net.journal.put("route_binding", route, {**value, "plan": plan}, immutable=True)
    warnings = [
        {
            "route": route["route"],
            "message": "Quoted transport fee exceeds warning threshold",
            "fee": route["min_input"] - route["output"],
            "threshold_bps": threshold,
        }
        for route in routes
        if (route["min_input"] - route["output"]) * 10000 > route["output"] * threshold
    ]
    return {
        "plan": plan,
        "principal": str(request["amount"]),
        "local": str(local),
        "routes": routes,
        "expected_transport_cost": sum(
            route["min_input"] - route["output"] for route in routes
        ),
        "maximum_transport_cost": sum(
            route["input"] - route["output"] for route in routes
        ),
        "warnings": warnings,
    }


def request_task(net, name, amount, purpose=None, observe=True):
    purpose = purpose or hex_bytes(digest(b"local cross-chain task"))
    result = net.chains[name].create_task(amount, purpose)
    task_id, request_id = result["task"], result["request"]
    if observe:
        evidence = synchronize(net, name, task=task_id, request=request_id)
        wait_observation(net, name, evidence["snapshot"]["sequence"])
    return {
        "chain": name,
        "task": task_id,
        "request": request_id,
        "amount": str(amount),
    }


def agent_action(net, name, task, action, amount=None):
    """Send a single locally signed agent action, then refresh its evidence."""
    task = identity(task)
    result = net.chains[name].agent_action(task, action, amount)
    evidence = synchronize(net, name, task=task)
    wait_observation(net, name, evidence["snapshot"]["sequence"])
    return {"result": result, "task": task, "snapshot": evidence["snapshot"]}


def fund(net, name, amount, invoice=0, intent_id=None):
    if not 0 < amount <= 2**64 - 1:
        raise ValueError("funding amount must be positive u64 USDC base units")
    intent_id = secrets.token_hex(16) if intent_id is None else intent_id
    if (
        not isinstance(intent_id, str)
        or not 1 <= len(intent_id) <= 128
        or not all(
            character.isascii() and (character.isalnum() or character in "-_.:")
            for character in intent_id
        )
    ):
        raise ValueError(
            "funding intent ID must contain 1 to 128 ASCII letters, digits, or -_.:"
        )
    binding = net.config["chains"][name]
    effect_id = f"funding:{intent_id}"
    intent = {
        "chain": name,
        "domain": identity(binding["domain"]),
        "vault": binding["vault"],
        "amount": str(amount),
        "invoice_id": identity(invoice),
        "effect_id": effect_id,
        "native_id": hex_bytes(digest(effect_id.encode())),
    }
    net.journal.put("funding_intent", intent_id, intent, immutable=True)
    print(
        f"Funding intent: {intent_id}; resume this payment with --id {intent_id}",
        file=sys.stderr,
        flush=True,
    )
    if net.journal.effect(effect_id) is None and intent["invoice_id"] != ZERO:
        invoice_number = int(intent["invoice_id"], 16)
        if invoice_number > 2**64 - 1:
            raise ValueError("global invoice ID must fit u64")
        invoice_record = net.view("get_invoice", invoice=invoice_number)
        if not invoice_record or invoice_record.get("closed") is not True:
            raise ValueError(
                "global invoice must exist and be closed before linked funding"
            )
    if net.journal.begin_effect(effect_id, intent):
        try:
            receipt = net.chains[name].fund(intent)
            previous = net.journal.get("funding_native", intent_id)
            if previous is not None and previous["payload"] != receipt["payload"]:
                raise ValueError("immutable funding receipt changed")
            net.journal.put(
                "funding_native", intent_id, previous or receipt, immutable=True
            )
        except Exception as error:
            net.journal.note_unknown(effect_id, error)
            raise PendingEvidence(
                f"funding {intent_id} requires receipt reconciliation; "
                "its token transfer will not be repeated"
            ) from error
    relayer = Relayer(net)
    relayer.reconcile_funding(intent_id)
    effect = net.journal.effect(effect_id)
    if effect["state"] != "Confirmed":
        raise PendingEvidence(
            f"funding {intent_id} has no confirmed native receipt; "
            "its token transfer will not be repeated"
        )
    receipt = effect["result"]
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        result = net.observe_funding(name, receipt)
        if result["state"] == "Confirmed":
            break
        time.sleep(0.5)
    else:
        raise PendingEvidence(
            f"funding {intent_id} is received locally; coordinator acknowledgement pending"
        )
    evidence = synchronize(net, name)
    wait_observation(net, name, evidence["snapshot"]["sequence"])
    return {**receipt, "intent_id": intent_id, "acknowledged": True}
