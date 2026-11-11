"""Maintained acceptance scenarios against all three real local RPC nodes."""

import time

from chains import PendingEvidence
from protocol import digest, identity, raw_id
from providers import LiquidityClient
from workflows import (
    UNIT,
    agent_action,
    apply_request,
    fund,
    request_task,
    synchronize,
    wait_observation,
)


def wait_until(description, read, predicate, timeout=600):
    deadline = time.monotonic() + timeout
    last = None
    report = 0
    while time.monotonic() < deadline:
        try:
            last = read()
            if predicate(last):
                return last
        except (OSError, PendingEvidence) as error:
            last = {"pending": str(error)}
        if time.monotonic() >= report:
            print(f"Waiting for {description}", flush=True)
            report = time.monotonic() + 30
        time.sleep(1)
    raise PendingEvidence(f"{description} was not confirmed; last observation: {last}")


def check(net):
    health = LiquidityClient(net.config["provider_url"]).health()
    assert health["local_mock"] and health["coordinator"] == net.coordinator
    result = {}
    for name, binding in net.config["chains"].items():
        evidence = synchronize(net, name)
        observed = wait_observation(net, name, evidence["snapshot"]["sequence"])
        assert observed["vault"] == binding["vault"]
        assert observed["asset"] == binding["asset"]
        if int(observed["sequence"]) == evidence["snapshot"]["sequence"]:
            assert int(observed["free"]) == evidence["snapshot"]["free"]
            assert int(observed["held"]) == evidence["snapshot"]["held"]
        result[name] = {
            "sequence": observed["sequence"],
            "checkpoint_sequence": evidence["snapshot"]["sequence"],
            "free": observed["free"],
            "held": observed["held"],
            "rpc": binding["rpc_url"],
        }
    return {
        "verified": True,
        "local_provider": health,
        "domains": result,
        "unresolved_payments": net.journal.unresolved() + health["unresolved_payments"],
    }


def smoke(net, restart_services, suspend_relayer):
    """Three real routes, bounded faults, service restart and local agent lifecycle.

    Existing tasks/plans are never reset. This scenario creates its own tasks,
    uses only free funds and releases its complete grants back to the Project.
    The local transport retains its configured fees.
    """
    provider = LiquidityClient(net.config["provider_url"])

    results = []
    for destination, source, fault in (
        ("ethereum", "solana", "partial"),
        ("near", "ethereum", "late_refund"),
        ("solana", "near", "false_success"),
    ):
        queue = net.view("get_state")
        if queue["queue_head"] != queue["queue_length"]:
            raise ValueError(
                "smoke requires an idle approval queue; settle its existing head "
                "and advance removed entries before running the scenario"
            )
        before = check(net)
        if any(int(value["held"]) for value in before["domains"].values()):
            raise ValueError(
                "smoke requires existing plans to settle first; no user holds were modified"
            )
        free_before = sum(value["free"] for value in before["domains"].values())
        migrated = 2 * UNIT if destination == "ethereum" else UNIT
        amount = before["domains"][destination]["free"] + migrated
        task = request_task(net, destination, amount, observe=False)
        binding = net.config["chains"][destination]
        wait_until(
            "automatic discovery of locally created task",
            lambda: net.view(
                "get_request",
                domain=raw_id(binding["domain"]),
                request=raw_id(task["request"]),
            ),
            lambda request: request is not None and request["amount"] == amount,
        )
        net.call(
            "approve", domain=raw_id(binding["domain"]), request=raw_id(task["request"])
        )
        # The provider cannot start a normal payment between Apply and fault
        # configuration: only the suspended relayer can submit the new commands.
        with suspend_relayer():
            plan = apply_request(net, destination, task["request"], source=source)
            if "plan" not in plan or len(plan["routes"]) != 1:
                raise ValueError(f"smoke needs one affordable migration: {plan}")
            plan_id = raw_id(plan["plan"])
            route_id = plan["routes"][0]["route"]
            provider.fault(route_id, fault)
        quote_response = net.journal.get("route_quote", route_id)
        if fault == "partial":
            wait_until(
                "partial real destination payment",
                lambda: provider.status(quote_response),
                lambda quote: int(quote["swapDetails"]["amountOut"]) == migrated // 2,
            )
        elif fault == "false_success":
            wait_until(
                "provider SUCCESS without destination tokens",
                lambda: provider.status(quote_response),
                lambda quote: (
                    quote["status"] == "SUCCESS"
                    and int(quote["swapDetails"]["amountOut"]) == 0
                ),
            )
        else:
            wait_until(
                "grant before late buffer refund",
                lambda: net.view("get_plan", plan=plan_id),
                lambda current: current["status"] == "Funded",
            )

        checkpoint = synchronize(
            net, destination, task=task["task"], request=task["request"]
        )
        wait_observation(net, destination, checkpoint["snapshot"]["sequence"])
        expected = amount if fault == "late_refund" else 0
        assert checkpoint["snapshot"]["available"] == expected, (
            "partial/status-only grant escaped custody checks"
        )
        if fault == "late_refund":
            current = net.view("get_plan", plan=plan_id)
            assert current["routes"][0]["confirmed_transport_expense"] is None
        if fault == "partial":
            # Restart with half the output already transferred. Journal replay
            # must preserve that tranche and never pay it a second time.
            restart_services()
            wait_until(
                "provider restart",
                provider.health,
                lambda health: (
                    health.get("coordinator") == net.coordinator
                    and health.get("local_mock") is True
                ),
                timeout=120,
            )
        provider.fault(route_id, fault, release=True)
        settled = wait_until(
            "full grant and actual buffer refund",
            lambda: net.view("get_plan", plan=plan_id),
            lambda current: (
                current["status"] == "Funded"
                and current["routes"][0]["confirmed_transport_expense"] is not None
            ),
        )
        fee = settled["routes"][0]["confirmed_transport_expense"]
        granted = agent_action(net, destination, task["task"], "withdraw", UNIT)
        assert granted["snapshot"]["started"]
        returned = agent_action(net, destination, task["task"], "return", UNIT)
        assert returned["snapshot"]["available"] == amount - UNIT
        released = agent_action(
            net, destination, task["task"], "release", amount - UNIT
        )
        assert released["snapshot"]["available"] == 0
        finished = agent_action(net, destination, task["task"], "finalize")
        assert finished["snapshot"]["task_state"] == 3
        assert finished["snapshot"]["issued"] == finished["snapshot"]["recovered"]
        after = check(net)
        assert all(value["held"] == 0 for value in after["domains"].values())
        assert (
            sum(value["free"] for value in after["domains"].values())
            == free_before - fee
        )
        quote = provider.status(quote_response)
        assert int(quote["swapDetails"]["amountOut"]) == migrated
        assert (
            int(quote["swapDetails"]["refundedAmount"])
            == settled["routes"][0]["expected_buffer_refund"]
        )
        result = {
            "source": source,
            "destination": destination,
            "task": task["task"],
            "request": task["request"],
            "plan": identity(plan_id),
            "fault": fault,
            "principal": amount,
            "migrated": migrated,
            "fee": fee,
            "expense": 0,
            "service_restart": fault == "partial",
        }
        net.journal.put("smoke_result", plan_id, result, immutable=True)
        results.append(result)
        print(
            f"Verified {source} -> {destination}: {fault}, full allocation, return and finalization",
            flush=True,
        )
    funding = invoice_smoke(net)
    return {
        "passed": True,
        "routes": results,
        "funding": funding,
        "unresolved_payments": net.journal.unresolved()
        + provider.health()["unresolved_payments"],
    }


def invoice_smoke(net):
    """Actual owner deposits, foreign funding proofs and replay across all chains."""
    invoice = net.call(
        "open_invoice",
        amount=3 * UNIT,
        purpose=digest(b"native funding acceptance").hex(),
    )
    net.call(
        "respond_invoice",
        role="owner",
        invoice=invoice,
        intended=3 * UNIT,
        reference=None,
    )
    initial = net.view("get_invoice", invoice=invoice)
    assert initial["actual_funding"] == 0, "invoice intent was credited as money"
    receipts = []
    for name, binding in net.config["chains"].items():
        before = check(net)["domains"][name]["free"]
        intent = f"smoke-invoice-{invoice}-{name}"
        receipt = fund(net, name, UNIT, invoice, intent_id=intent)
        repeated = fund(net, name, UNIT, invoice, intent_id=intent)
        assert repeated["payload"] == receipt["payload"]
        actual = net.view("get_domain", domain=raw_id(binding["domain"]))
        assert actual["free"] == before + UNIT, (
            "same funding intent caused another transfer"
        )
        receipts.append(
            {"chain": name, "funding_id": receipt["funding_id"], "intent_id": intent}
        )
    invoice_view = net.view("get_invoice", invoice=invoice)
    assert invoice_view["actual_funding"] == 3 * UNIT, (
        "funding receipt replay changed invoice accounting"
    )
    print(
        "Verified actual three-chain invoice funding and same-intent replay", flush=True
    )
    return {
        "invoice": invoice,
        "actual": invoice_view["actual_funding"],
        "receipts": receipts,
    }
