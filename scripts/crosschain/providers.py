"""Persistent local 1Click transport and MPC provider simulation.

These are explicitly development providers. They use real local tokens and
native chain evidence; their own HTTP status never authorizes a Project grant.
"""

import json
import time
from datetime import datetime, timezone
from functools import wraps
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import RLock
from urllib.parse import parse_qs, urlencode, urlsplit

from chains import PendingEvidence, http_json
from protocol import (
    Action,
    CommandState,
    canonical_json,
    decode_checkpoint,
    decode_command,
    digest,
    foreign_hash,
    hex_bytes,
    identity,
    private_key,
    raw_id,
    sign_mpc,
    unhex,
)

SETTLEMENT_MODES = {"normal", "partial", "refund", "false_success", "late_refund"}


def iso_time(seconds):
    return (
        datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")
    )


def deadline_seconds(value):
    return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp())


class LiquidityClient:
    """HTTP boundary used by application workflows and local acceptance scenarios.

    Quotes and status use the 1Click wire shape. Fault controls and loopback
    transport belong to this local deployment; callers never read provider storage.
    """

    def __init__(self, url):
        self.url = url.rstrip("/")

    def health(self):
        return http_json(self.url + "/health")

    def quote(self, request):
        response = http_json(self.url + "/v0/quote", request)
        if response["quoteRequest"] != request:
            raise ValueError("provider quote changed the requested route terms")
        quote = response["quote"]
        amounts = {}
        for field in ("amountIn", "minAmountIn", "amountOut", "minAmountOut"):
            value = quote[field]
            if (
                not isinstance(value, str)
                or not value.isascii()
                or not value.isdecimal()
                or not 0 < int(value) <= (1 << 64) - 1
            ):
                raise ValueError(f"invalid quoted {field}")
            amounts[field] = int(value)
        if not (
            amounts["amountIn"] >= amounts["minAmountIn"] >= amounts["amountOut"]
            and amounts["amountOut"]
            == amounts["minAmountOut"]
            == int(request["amount"])
            and quote["deadline"] == request["deadline"]
        ):
            raise ValueError("provider quote does not preserve exact-output terms")
        if not request["dry"] and (
            response["correlationId"] != request["correlationId"]
            or quote["depositMemo"] != request["correlationId"]
            or not isinstance(quote["depositAddress"], str)
            or not quote["depositAddress"]
        ):
            raise ValueError("provider quote has no exact deposit identity")
        return response

    def status(self, quote_response):
        quote = quote_response["quote"]
        query = urlencode(
            {
                "depositAddress": quote["depositAddress"],
                "depositMemo": quote["depositMemo"],
            }
        )
        return http_json(self.url + "/v0/status?" + query)

    def fault(self, quote_id, mode, release=False):
        return http_json(
            self.url + "/local/fault",
            {"quote_id": quote_id, "mode": mode, "release": release},
        )


def serialized(method):
    """Keep HTTP and background read/modify/write transitions in one order."""

    @wraps(method)
    def call(self, *args, **kwargs):
        with self._lock:
            return method(self, *args, **kwargs)

    return call


class Transport:
    def __init__(self, journal, assets, fee=100_000, refund_fee=10_000):
        self.journal = journal
        self.assets = assets
        self.fee = fee
        self.refund_fee = refund_fee
        self._lock = RLock()

    @serialized
    def quote(self, request):
        for field, expected in (
            ("swapType", "EXACT_OUTPUT"),
            ("depositType", "ORIGIN_CHAIN"),
            ("recipientType", "DESTINATION_CHAIN"),
            ("refundType", "ORIGIN_CHAIN"),
        ):
            if request.get(field) != expected:
                raise ValueError(f"local transport requires {field}={expected}")
        source = self.assets[request["originAsset"]]
        destination = self.assets[request["destinationAsset"]]
        if source["chain"] == destination["chain"]:
            raise ValueError("transport quote requires different chains")
        output = int(request["amount"])
        slippage = int(request.get("slippageTolerance", 0))
        expiry = deadline_seconds(request["deadline"])
        if not 0 < output <= (1 << 63) or not 0 <= slippage <= 1000:
            raise ValueError("invalid amount/slippage")
        if expiry <= int(time.time()) or expiry > int(time.time()) + 3600:
            raise ValueError("local quote deadline must be within one hour")
        if not request.get("recipient") or not request.get("refundTo"):
            raise ValueError("exact recipient and refund custody required")
        quote_id = request.get("correlationId") or hex_bytes(
            digest(canonical_json(request))
        )
        existing = self.journal.get("quote", quote_id)
        if existing:
            if existing["request"] != request:
                raise ValueError(
                    "quote correlation ID reused with different parameters"
                )
            return existing["response"]
        minimum = output + self.fee
        maximum = minimum + (minimum * slippage + 9999) // 10000
        if maximum > (1 << 64) - 1:
            raise ValueError("quote exceeds shared u64 limit")
        quote = {
            "amountIn": str(maximum),
            "minAmountIn": str(minimum),
            "amountOut": str(output),
            "minAmountOut": str(output),
            "refundFee": str(self.refund_fee),
            "withdrawFee": "0",
            "deadline": request["deadline"],
            "timeEstimate": 1,
        }
        if not request.get("dry", False):
            # Shared LP recipient + unique memo is a local mock simplification;
            # source dispatch receipts, not memo/status claims, attribute cash.
            quote.update(depositAddress=source["deposit_address"], depositMemo=quote_id)
        response = {
            "quoteRequest": request,
            "quote": quote,
            "correlationId": quote_id,
            "timestamp": iso_time(time.time()),
            "localMock": True,
        }
        if request.get("dry", False):
            return response
        self.journal.put(
            "quote",
            quote_id,
            {
                "request": request,
                "response": response,
                "status": "PENDING_DEPOSIT",
                "updated": iso_time(time.time()),
                "mode": "normal",
                "released": False,
                "delivered": "0",
                "refunded": "0",
                "deposited": "0",
            },
            immutable=True,
        )
        return response

    @serialized
    def status(self, address, memo):
        record = self.journal.get("quote", memo)
        if record is None or record["response"]["quote"]["depositAddress"] != address:
            raise ValueError("exact deposit address and memo must identify one quote")
        return {
            "correlationId": memo,
            "quoteResponse": record["response"],
            "status": record["status"],
            "updatedAt": record["updated"],
            "swapDetails": {
                "depositedAmount": record["deposited"],
                "amountIn": str(int(record["deposited"]) - int(record["refunded"])),
                "amountOut": record["delivered"],
                "refundedAmount": record["refunded"],
            },
            "localMock": True,
        }

    @serialized
    def fault(self, quote_id, mode, release=False):
        if not isinstance(release, bool):
            raise ValueError("release must be a boolean")
        if mode not in SETTLEMENT_MODES:
            raise ValueError("unsupported local fault mode")
        record = self.journal.get("quote", quote_id)
        if record is None:
            raise ValueError("unknown quote")
        payment_started = (
            record.get("payment_started", False)
            or int(record["delivered"])
            or int(record["refunded"])
        )
        if payment_started and mode != record["mode"]:
            raise ValueError(
                "cannot change settlement mode after a native payment attempt"
            )
        if record["released"] and not release:
            raise ValueError("a released fault cannot be paused again")
        record.update(mode=mode, released=release, updated=iso_time(time.time()))
        self.journal.put("quote", quote_id, record)
        return {"quote": quote_id, "mode": mode, "released": release}

    @serialized
    def progress(self, quote_id, confirmed_dispatch, pay):
        """Called with independently verified native dispatch, never HTTP status.

        pay(kind, tranche, amount, target_total) durably journals and either
        confirms a real native transfer or raises PendingEvidence. Retrying this
        function cannot create a second payment for an unknown tranche.
        """
        record = self.journal.get("quote", quote_id)
        if record is None or record["request"].get("dry"):
            raise ValueError("missing executable quote")
        quote = record["response"]["quote"]
        input_amount, output = int(quote["amountIn"]), int(quote["amountOut"])
        if confirmed_dispatch != input_amount:
            raise ValueError("source dispatch does not match full quoted input")
        mode, released = record["mode"], record["released"]
        if mode not in SETTLEMENT_MODES:
            raise ValueError("unsupported local settlement mode")
        record["deposited"] = str(input_amount)
        record["status"] = "PROCESSING"
        self.journal.put("quote", quote_id, record)

        def transfer(kind, tranche, amount, target_total):
            # Persist before the callback: an exception may mean broadcast succeeded.
            record["payment_started"] = True
            self.journal.put("quote", quote_id, record)
            return pay(kind, tranche, amount, target_total)

        if mode == "false_success" and not released:
            record["status"] = "SUCCESS"
        elif mode == "refund":
            refund = max(0, input_amount - int(quote["refundFee"]))
            if int(record["refunded"]) < refund:
                transfer("refund", "failed", refund, refund)
                record["refunded"] = str(refund)
            record["status"] = "REFUNDED"
        else:
            first = output // 2 if mode == "partial" else output
            if int(record["delivered"]) < first:
                transfer("delivery", "first", first, first)
                record["delivered"] = str(first)
                self.journal.put("quote", quote_id, record)
            if mode == "partial" and not released:
                return
            if int(record["delivered"]) < output:
                remainder = output - int(record["delivered"])
                transfer("delivery", "remainder", remainder, output)
                record["delivered"] = str(output)
                self.journal.put("quote", quote_id, record)
            record["status"] = "SUCCESS"
            refund = input_amount - int(quote["minAmountIn"])
            if mode != "late_refund" or released:
                if int(record["refunded"]) < refund:
                    transfer("refund", "buffer", refund, refund)
                    record["refunded"] = str(refund)
        record["updated"] = iso_time(time.time())
        self.journal.put("quote", quote_id, record)


def provider_server(transport, address, health, *, container=False):
    """Local provider API; fault injection is restricted to loopback clients."""

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format, *_args):
            return

        def response(self, status, value):
            raw = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            path = urlsplit(self.path)
            try:
                if path.path == "/health":
                    result = health()
                elif path.path == "/v0/status":
                    query = parse_qs(path.query)
                    result = transport.status(
                        query["depositAddress"][0], query["depositMemo"][0]
                    )
                else:
                    return self.response(404, {"error": "unknown local endpoint"})
                self.response(200, result)
            except (ValueError, KeyError) as error:
                self.response(400, {"error": str(error)})

        def do_POST(self):
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 65536:
                    raise ValueError("invalid request size")
                body = json.loads(self.rfile.read(size))
                if self.path == "/v0/quote":
                    result = transport.quote(body)
                elif self.path == "/local/fault":
                    if self.client_address[0] != "127.0.0.1":
                        return self.response(
                            403, {"error": "fault injection requires local access"}
                        )
                    result = transport.fault(
                        body["quote_id"], body["mode"], body.get("release", False)
                    )
                else:
                    return self.response(404, {"error": "unknown local endpoint"})
                self.response(200, result)
            except (ValueError, KeyError) as error:
                self.response(400, {"error": str(error)})

    if address[0] != "127.0.0.1" and not (container and address[0] == "0.0.0.0"):
        raise ValueError(
            "mock providers require loopback or explicit container binding"
        )
    server = ThreadingHTTPServer(address, Handler)
    server.daemon_threads = True
    return server


class MpcProvider:
    def __init__(self, network, key_directory):
        self.network = network
        self.sign_key = private_key(key_directory / "sign.bin")
        self.read_key = private_key(key_directory / "read.bin")

    def tick(self):
        net = self.network
        state = net.near.near_view(net.mpc, "get_state")
        for request_id in range(int(state["next_request"])):
            recorded = net.journal.get("provider_request", str(request_id), {})
            if recorded.get("state") in ("Answered", "Closed"):
                continue
            pending = net.near.near_view(
                net.mpc, "get_pending", {"request_id": request_id}
            )
            if not pending:
                continue
            if pending["answered"]:
                # A response or timeout can close the yield while its RPC result
                # is lost. Closure is not proof the coordinator accepted a signature.
                net.journal.put(
                    "provider_request",
                    str(request_id),
                    {"state": "Closed", "kind": pending["kind"], "reconciled": True},
                )
                continue
            try:
                expected = pending["digest"]
                if pending["kind"] == "sign":
                    matches = [
                        record
                        for record in net.commands()
                        if raw_id(record["digest"]) == raw_id(expected)
                    ]
                    if len(matches) != 1:
                        raise ValueError(
                            "signer found no unique finalized coordinator command"
                        )
                    command = matches[0]
                    raw = command["command"]
                    if digest(unhex(raw)) != unhex(expected, 32):
                        raise ValueError("stored coordinator command digest mismatch")
                    parsed = decode_command(raw)
                    net.binding(parsed["domain"])
                    if pending["request"]["path"] != net.config["sign_path"]:
                        raise ValueError("wrong coordinator derivation path")
                    if int(time.time()) > parsed["expires"]:
                        raise PendingEvidence(
                            "coordinator command expired before signing"
                        )
                    response = sign_mpc(self.sign_key, expected)
                elif pending["kind"] == "verify":
                    request = pending["request"]["request"]
                    chain = {"Ethereum": "ethereum", "Solana": "solana"}[
                        next(iter(request))
                    ]
                    values = net.chains[chain].foreign_evidence(
                        request, net.config["chains"][chain]
                    )
                    actual = foreign_hash(request, values)
                    if unhex(actual, 32) != unhex(expected, 32):
                        raise ValueError(
                            "actual RPC evidence does not match requested payload hash"
                        )
                    response = {
                        "payload_hash": raw_id(actual),
                        "signature": sign_mpc(self.read_key, actual),
                    }
                else:
                    raise ValueError("unknown provider request kind")
                accepted = net.near.bridge(
                    "call",
                    role="provider",
                    contract=net.mpc,
                    method="respond",
                    args={"request_id": request_id, "response": response},
                    deposit="0",
                    wait=True,
                )
                if accepted is not True:
                    raise PendingEvidence(
                        "local MPC did not accept the response; coordinator result remains unconfirmed"
                    )
                net.journal.put(
                    "provider_request",
                    str(request_id),
                    {"state": "Answered", "kind": pending["kind"]},
                )
            except Exception as error:
                net.journal.put(
                    "provider_request",
                    str(request_id),
                    {
                        "state": "Unverified",
                        "kind": pending["kind"],
                        "error": str(error),
                        "updated": int(time.time()),
                    },
                )


class TransportProvider:
    def __init__(self, network, transport):
        self.network = network
        self.transport = transport

    def pay_route(self, name, route, kind, amount, effect_id, target_total):
        net = self.network
        request = {"chain": name, "route": route, "kind": kind, "amount": str(amount)}
        if not net.journal.begin_effect(effect_id, request):
            effect = net.journal.effect(effect_id)
            if effect["state"] == "Confirmed":
                return effect["result"]
            state = net.chains[name].route_view(route, kind)
            total = int(state["received"]) + int(state["escrow_balance"])
            if total >= target_total:
                net.journal.settle_effect(
                    effect_id, {"reconciled": True, "route": route, "total": str(total)}
                )
                return net.journal.effect(effect_id)["result"]
            net.journal.note_unknown(
                effect_id,
                "No affirmative finalized payment evidence; fresh payment withheld",
            )
            raise PendingEvidence("previous provider payment remains unknown")
        try:
            result = net.chains[name].deposit_route(route, kind, amount, effect_id)
            net.journal.settle_effect(effect_id, result)
            return result
        except Exception as error:
            net.journal.note_unknown(effect_id, error)
            raise

    def verify_dispatch(self, route_id, metadata, evidence):
        """Bind native custody evidence to the coordinator command and exact LP quote."""
        net = self.network
        net.verify_evidence(evidence)
        snapshot = decode_checkpoint(evidence["payload"])
        plan = net.view("get_plan", plan=raw_id(metadata["plan"]))
        if plan is None or raw_id(plan["plan"]) != raw_id(metadata["plan"]):
            raise ValueError("missing authoritative transport plan")
        matches = [
            route
            for route in plan["routes"]
            if raw_id(route["quote"]["route"]) == raw_id(route_id)
        ]
        if len(matches) != 1:
            raise ValueError("route is not unique in the authoritative plan")
        route = matches[0]
        quote = route["quote"]
        source, source_binding = net.binding(quote["source"])
        destination, destination_binding = net.binding(plan["destination"])
        if (
            metadata["source"] != source
            or metadata["destination"] != destination
            or evidence["chain"] != source
        ):
            raise ValueError("route chain provenance mismatch")
        stored = net.journal.get("quote", metadata["quote_id"])
        if stored is None:
            raise ValueError("missing provider quote")
        response = stored["response"]
        if (
            stored["request"] != response["quoteRequest"]
            or metadata["quote"] != response
            or digest(canonical_json(response)).hex() != raw_id(quote["quote_hash"])
        ):
            raise ValueError("provider quote differs from coordinator commitment")
        details, request = response["quote"], response["quoteRequest"]
        if (
            response["correlationId"] != metadata["quote_id"]
            or details["depositMemo"] != metadata["quote_id"]
            or details["depositAddress"] != source_binding["deposit_address"]
            or raw_id(quote["deposit"]) != raw_id(source_binding["deposit_word"])
            or request["originAsset"] != source_binding["asset_id"]
            or request["destinationAsset"] != destination_binding["asset_id"]
            or int(details["amountIn"]) != int(quote["input"])
            or int(details["minAmountIn"]) != int(quote["min_input"])
            or int(details["amountOut"]) != int(quote["output"])
            or request["recipient"] != quote["recipient"]
            or request["refundTo"] != quote["refund"]
        ):
            raise ValueError("quoted transfer terms differ from authorized route")
        for chain, recipient, kind in (
            (destination, quote["recipient"], "delivery"),
            (source, quote["refund"], "refund"),
        ):
            adapter = net.chains[chain]
            custody = adapter.route_address(route_id, kind)
            if adapter.recipient_identity(recipient) != adapter.recipient_identity(
                custody
            ):
                raise ValueError("quote does not target registered route custody")
        if route["dispatch"] is None:
            raise PendingEvidence("coordinator has not authorized source dispatch")
        record = net.view("get_command", operation=raw_id(route["dispatch"]))
        if record is None:
            raise ValueError("source dispatch command is missing")
        command = decode_command(record["command"])
        command_hash = digest(unhex(record["command"]))
        if (
            command_hash != unhex(record["digest"], 32)
            or raw_id(record["operation"]) != raw_id(route["dispatch"])
            or command["operation"] != identity(route["dispatch"])
            or command["action"] != Action.DISPATCH
            or command["domain"] != identity(quote["source"])
            or command["related"] != identity(plan["plan"])
            or command["reference"] != identity(route_id)
            or command["amount"] != int(quote["input"])
            or command["recipient"] != identity(source_binding["deposit_word"])
        ):
            raise ValueError("source command does not authorize this quoted LP payment")
        if (
            snapshot["domain"] != command["domain"]
            or snapshot["operation"] != command["operation"]
            or snapshot["command_digest"] != hex_bytes(command_hash)
            or snapshot["command_action"] != Action.DISPATCH
            or snapshot["command_state"] != CommandState.SUCCEEDED
            or int(snapshot["command_result"], 16) != command["amount"]
            or snapshot["plan"] != command["related"]
            or snapshot["route_plan"] != command["related"]
            or snapshot["route"] != command["reference"]
            or snapshot["route_kind"] != 2
            or snapshot["route_expected"] != command["amount"]
            or snapshot["route_dispatched"] != command["amount"]
        ):
            raise ValueError(
                "native receipt does not prove the exact quoted source dispatch"
            )
        # Once actual dispatch is proved, expiry cannot erase the provider's obligation.
        return source, destination, snapshot["route_dispatched"]

    def discover_routes(self):
        """Match our own quotes to finalized plans, without relayer storage."""
        net = self.network
        for counter in range(1, int(net.view("get_state")["next_plan"])):
            plan_id = raw_id(identity(counter))
            diagnostic = f"plan:{plan_id}"
            try:
                plan = net.view("get_plan", plan=plan_id)
                if plan is None:
                    raise PendingEvidence("committed transport plan is not visible")
                destination, _ = net.binding(plan["destination"])
                for route in plan["routes"]:
                    quote = route["quote"]
                    route_id = raw_id(quote["route"])
                    stored = net.journal.get("quote", route_id)
                    if stored is None:
                        continue  # Another provider's quote creates no obligation here.
                    response = stored["response"]
                    if digest(canonical_json(response)).hex() != raw_id(
                        quote["quote_hash"]
                    ):
                        raise ValueError(
                            "authorized route differs from our quoted terms"
                        )
                    source, _ = net.binding(quote["source"])
                    net.journal.put(
                        "route_binding",
                        route_id,
                        {
                            "plan": plan_id,
                            "source": source,
                            "destination": destination,
                            "quote_id": route_id,
                            "quote": response,
                        },
                        immutable=True,
                    )
                net.journal.resolve("transport_error", diagnostic)
            except Exception as error:
                net.journal.put(
                    "transport_error",
                    diagnostic,
                    {"error": str(error), "at": int(time.time())},
                )

    def dispatch_evidence(self, route_id, metadata):
        """Read source custody only after NEAR confirms its dispatch command."""
        net = self.network
        evidence = net.journal.get("dispatch_evidence", route_id)
        if evidence is not None:
            return evidence
        plan = net.view("get_plan", plan=raw_id(metadata["plan"]))
        route = next(
            item
            for item in plan["routes"]
            if raw_id(item["quote"]["route"]) == route_id
        )
        operation = route["dispatch"]
        if operation is None:
            return None
        command = net.view("get_command", operation=raw_id(operation))
        if command is None or int(command["state"]) != CommandState.SUCCEEDED:
            return None
        evidence = net.checkpoint(
            metadata["source"],
            operation=identity(operation),
            plan=identity(metadata["plan"]),
            route=identity(route_id),
            kind="refund",
        )
        self.verify_dispatch(route_id, metadata, evidence)
        net.journal.put("dispatch_evidence", route_id, evidence, immutable=True)
        return evidence

    def tick(self):
        net = self.network
        self.discover_routes()
        for route_id, metadata in net.journal.records("route_binding"):
            try:
                if self.obligations_settled(route_id, metadata):
                    net.journal.resolve("transport_error", route_id)
                    continue
                evidence = self.dispatch_evidence(route_id, metadata)
                if evidence is None:
                    continue
                source, destination, dispatched = self.verify_dispatch(
                    route_id, metadata, evidence
                )

                def pay(kind, tranche, amount, total):
                    name = destination if kind == "delivery" else source
                    return self.pay_route(
                        name,
                        identity(route_id),
                        kind,
                        amount,
                        f"transport:{route_id}:{kind}:{tranche}",
                        total,
                    )

                self.transport.progress(metadata["quote_id"], dispatched, pay)
                net.journal.resolve("transport_error", route_id)
            except Exception as error:
                net.journal.put(
                    "transport_error",
                    route_id,
                    {"error": str(error), "at": int(time.time())},
                )

    def obligations_settled(self, route_id, metadata):
        """Skip RPC work only after every real financial tranche is confirmed."""
        journal = self.network.journal
        record = journal.get("quote", metadata["quote_id"])
        if record is None or record.get("response") != metadata["quote"]:
            return False
        quote = record["response"]["quote"]
        mode = record.get("mode")
        if mode not in SETTLEMENT_MODES:
            return False
        payments = []
        input_amount, output = int(quote["amountIn"]), int(quote["amountOut"])
        if mode == "refund":
            refund = max(0, input_amount - int(quote["refundFee"]))
            if (
                record.get("status") != "REFUNDED"
                or int(record["delivered"]) != 0
                or int(record["refunded"]) != refund
            ):
                return False
            payments.append(("refund", "failed", refund))
        else:
            refund = input_amount - int(quote["minAmountIn"])
            if (
                record.get("status") != "SUCCESS"
                or int(record["delivered"]) != output
                or int(record["refunded"]) != refund
            ):
                return False
            first = output // 2 if mode == "partial" else output
            payments.extend(
                (
                    ("delivery", "first", first),
                    ("delivery", "remainder", output - first),
                    ("refund", "buffer", refund),
                )
            )
        for kind, tranche, amount in payments:
            if amount == 0:
                continue
            effect = journal.effect(f"transport:{route_id}:{kind}:{tranche}")
            expected = {
                "chain": metadata["destination"]
                if kind == "delivery"
                else metadata["source"],
                "route": identity(route_id),
                "kind": kind,
                "amount": str(amount),
            }
            if (
                effect is None
                or effect["state"] != "Confirmed"
                or effect["request"] != expected
                or effect["result"] is None
            ):
                return False
        return not any(
            effect["request"].get("route") == identity(route_id)
            for effect in journal.unresolved()
        )
