"""Provider payments require an exact coordinator route and native dispatch proof."""

import copy
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chains import Chain, PendingEvidence
from journal import Journal
from protocol import (
    CHECKPOINT_FIELDS,
    CHECKPOINT_IDENTITIES,
    ZERO,
    Action,
    CommandState,
    canonical_json,
    decode_checkpoint,
    digest,
    encode_command,
    hex_bytes,
    identity,
    raw_id,
)
from providers import TransportProvider
from worker import Relayer


def checkpoint(fields):
    words = []
    for field in CHECKPOINT_FIELDS:
        value = fields.get(field, ZERO if field in CHECKPOINT_IDENTITIES else 0)
        words.append(
            bytes.fromhex(raw_id(value))
            if field in CHECKPOINT_IDENTITIES
            else int(value).to_bytes(32, "big")
        )
    return hex_bytes(b"".join(words))


class TransportFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.journal = Journal(Path(temporary.name) / "provider.sqlite3")
        self.app_journal = Journal(Path(temporary.name) / "application.sqlite3")
        self.source, self.destination = identity(1), identity(2)
        self.plan_id, self.route_id, self.operation = (
            identity(1),
            identity(4),
            identity(5),
        )
        self.bindings = {
            "ethereum": {
                "domain": self.source,
                "vault": "source-vault",
                "asset_id": "local:ethereum:usdc",
                "deposit_word": identity(0x1234567890ABCDEF1234567890ABCDEF12345678),
                "deposit_address": "lp",
            },
            "solana": {
                "domain": self.destination,
                "vault": "destination-vault",
                "asset_id": "local:solana:usdc",
            },
        }
        self.request = {
            "originAsset": "local:ethereum:usdc",
            "destinationAsset": "local:solana:usdc",
            "recipient": "destination-escrow",
            "refundTo": "source-escrow",
        }
        self.response = {
            "quoteRequest": self.request,
            "correlationId": raw_id(self.route_id),
            "quote": {
                "amountIn": "110",
                "minAmountIn": "105",
                "amountOut": "100",
                "depositAddress": "lp",
                "depositMemo": raw_id(self.route_id),
            },
        }
        self.quote = {
            "route": raw_id(self.route_id),
            "source": raw_id(self.source),
            "input": 110,
            "min_input": 105,
            "output": 100,
            "deposit": raw_id(self.bindings["ethereum"]["deposit_word"]),
            "recipient": "destination-escrow",
            "refund": "source-escrow",
            "quote_hash": digest(canonical_json(self.response)).hex(),
            "expires": 1,
        }
        self.plan = {
            "plan": raw_id(self.plan_id),
            "status": "AwaitingDelivery",
            "requires_attention": False,
            "destination": raw_id(self.destination),
            "routes": [{"quote": self.quote, "dispatch": raw_id(self.operation)}],
        }
        self.command = {
            "domain": self.source,
            "action": Action.DISPATCH,
            "operation": self.operation,
            "related": self.plan_id,
            "reference": self.route_id,
            "amount": 110,
            "recipient": self.bindings["ethereum"]["deposit_word"],
            "expires": 1,
        }
        self.command_record = {}
        self.snapshot = {
            "domain": self.source,
            "operation": self.operation,
            "command_action": Action.DISPATCH,
            "command_state": 1,
            "command_result": identity(110),
            "plan": self.plan_id,
            "route_plan": self.plan_id,
            "route": self.route_id,
            "route_kind": 2,
            "route_expected": 110,
            "route_dispatched": 110,
        }
        self.update_command()
        self.metadata = {
            "source": "ethereum",
            "destination": "solana",
            "plan": raw_id(self.plan_id),
            "quote_id": raw_id(self.route_id),
            "quote": self.response,
        }
        self.journal.put(
            "quote",
            raw_id(self.route_id),
            {"request": self.request, "response": self.response},
        )
        self.route_view = Mock(
            return_value={"exists": True, "received": "0", "escrow_balance": "0"}
        )
        chains = {}
        for name, address in (
            ("ethereum", "source-escrow"),
            ("solana", "destination-escrow"),
        ):
            chain = Mock(
                spec=(
                    "route_address",
                    "route_view",
                    "deposit_route",
                    "execute_command",
                    "collect_route",
                )
            )
            chain.route_address.return_value = address
            chain.route_view = self.route_view
            chains[name] = chain
        self.network = SimpleNamespace(
            journal=self.journal,
            config={"chains": self.bindings},
            chains=chains,
            verify_evidence=Mock(),
            view=Mock(side_effect=self.view),
            binding=self.binding,
            checkpoint=Mock(
                side_effect=lambda *_args, **_kwargs: {
                    **self.evidence(),
                    "snapshot": decode_checkpoint(self.evidence()["payload"]),
                }
            ),
        )
        self.transport = SimpleNamespace(progress=Mock())
        self.provider = TransportProvider(self.network, self.transport)

    def binding(self, domain):
        for name, binding in self.bindings.items():
            if raw_id(domain) == raw_id(binding["domain"]):
                return name, binding
        raise ValueError("unregistered domain")

    def view(self, method, **args):
        if method == "get_state":
            return {"next_plan": 2}
        if method == "get_plan":
            return self.plan if raw_id(args["plan"]) == raw_id(self.plan_id) else None
        if method == "get_command":
            return (
                self.command_record
                if raw_id(args["operation"]) == raw_id(self.operation)
                else None
            )
        raise AssertionError(method)

    def update_command(self):
        raw = encode_command(self.command)
        self.command_record.update(
            command=hex_bytes(raw),
            digest=hex_bytes(digest(raw)),
            operation=raw_id(self.operation),
            state=1,
        )
        self.snapshot["command_digest"] = self.command_record["digest"]

    def evidence(self):
        return {"chain": "ethereum", "payload": checkpoint(self.snapshot)}


class TransportProvenanceTests(TransportFixture):
    def test_actual_dispatch_remains_payable_after_quote_expiry(self):
        self.assertEqual(
            self.provider.verify_dispatch(
                self.route_id, self.metadata, self.evidence()
            ),
            ("ethereum", "solana", 110),
        )
        self.network.verify_evidence.assert_called_once()

    def test_wrong_lp_recipient_is_rejected_even_with_matching_command_receipt(self):
        self.command["recipient"] = identity(9)
        self.update_command()
        with self.assertRaisesRegex(ValueError, "quoted LP payment"):
            self.provider.verify_dispatch(self.route_id, self.metadata, self.evidence())

    def test_unrelated_native_success_cannot_authorize_payment(self):
        original = self.snapshot.copy()
        for field, value in (
            ("operation", identity(8)),
            ("route_plan", identity(8)),
            ("route", identity(8)),
            ("command_action", Action.RESERVE),
            ("route_kind", 1),
            ("route_dispatched", 109),
            ("command_digest", identity(8)),
        ):
            with self.subTest(field=field):
                self.snapshot = {**original, field: value}
                with self.assertRaisesRegex(ValueError, "exact quoted source dispatch"):
                    self.provider.verify_dispatch(
                        self.route_id, self.metadata, self.evidence()
                    )

    def test_quote_hash_and_actual_receivers_are_independently_bound(self):
        changed = copy.deepcopy(self.metadata)
        changed["quote"]["quote"]["amountOut"] = "101"
        with self.assertRaisesRegex(ValueError, "commitment"):
            self.provider.verify_dispatch(self.route_id, changed, self.evidence())
        self.network.chains["solana"].route_address.return_value = "another-escrow"
        with self.assertRaisesRegex(ValueError, "registered route custody"):
            self.provider.verify_dispatch(self.route_id, self.metadata, self.evidence())

    def test_provider_discovers_and_verifies_without_relayer_journal(self):
        route_key = raw_id(self.route_id)
        self.assertNotEqual(self.journal.path, self.app_journal.path)
        for kind in ("quote", "plan", "route_binding", "dispatch_evidence"):
            self.assertEqual(self.app_journal.records(kind), [])
        self.assertEqual(self.journal.records("route_binding"), [])
        self.assertEqual(self.journal.records("dispatch_evidence"), [])

        # Authorization is not evidence of execution; the provider waits for acknowledgement.
        self.command_record["state"] = 0
        self.provider.tick()
        self.transport.progress.assert_not_called()
        self.network.checkpoint.assert_not_called()
        self.assertIsNone(self.journal.get("dispatch_evidence", route_key))
        self.command_record["state"] = 1
        self.provider.tick()
        self.assertEqual(self.journal.get("route_binding", route_key), self.metadata)
        self.assertEqual(
            self.journal.get("dispatch_evidence", route_key)["payload"],
            self.evidence()["payload"],
        )
        self.network.checkpoint.assert_called_once_with(
            "ethereum",
            operation=self.operation,
            plan=self.plan_id,
            route=self.route_id,
            kind="refund",
        )
        self.transport.progress.assert_called_once()
        quote_id, dispatched, pay = self.transport.progress.call_args.args
        self.assertEqual((quote_id, dispatched), (route_key, 110))
        with patch.object(self.provider, "pay_route") as payment:
            pay("delivery", "first", 100, 100)
        payment.assert_called_once_with(
            "solana",
            self.route_id,
            "delivery",
            100,
            f"transport:{route_key}:delivery:first",
            100,
        )

        # Application records neither bootstrap nor redirect provider payments.
        bad = {**self.metadata, "source": "solana"}
        self.app_journal.put("route_binding", route_key, bad)
        self.app_journal.put("dispatch_evidence", route_key, {"payload": "forged"})
        self.transport.progress.reset_mock()
        self.provider.tick()
        self.transport.progress.assert_called_once()
        self.assertEqual(self.journal.get("route_binding", route_key), self.metadata)
        self.assertEqual(self.app_journal.records("quote"), [])

        # The provider must still reject corruption of its own saved provenance.
        self.transport.progress.reset_mock()
        self.journal.put("route_binding", route_key, bad)
        self.provider.tick()
        self.transport.progress.assert_not_called()
        self.assertIn(
            "provenance", self.journal.get("transport_error", route_key)["error"]
        )


class CompletedTransportTests(TransportFixture):
    def setUp(self):
        super().setUp()
        self.route_key = raw_id(self.route_id)
        self.response["quote"]["refundFee"] = "2"
        self.quote["quote_hash"] = digest(canonical_json(self.response)).hex()
        self.record = {
            "request": self.request,
            "response": self.response,
            "mode": "normal",
            "status": "SUCCESS",
            "delivered": "100",
            "refunded": "5",
        }
        self.journal.put("quote", self.route_key, self.record)
        self.journal.put("route_binding", self.route_key, self.metadata)
        self.journal.put("dispatch_evidence", self.route_key, self.evidence())

    def payment(self, kind, tranche, amount, confirmed=True):
        key = f"transport:{self.route_key}:{kind}:{tranche}"
        self.journal.begin_effect(
            key,
            {
                "chain": "solana" if kind == "delivery" else "ethereum",
                "route": self.route_id,
                "kind": kind,
                "amount": str(amount),
            },
        )
        if confirmed:
            self.journal.settle_effect(key, {"native_receipt": key})

    def test_fully_paid_delivery_and_buffer_refund_skip_native_polling(self):
        self.payment("delivery", "first", 100)
        self.payment("refund", "buffer", 5)
        self.journal.put(
            "transport_error", self.route_key, {"error": "old timeout", "at": 1}
        )
        self.provider.tick()
        self.network.verify_evidence.assert_not_called()
        self.assertNotIn(
            "get_command", [call.args[0] for call in self.network.view.call_args_list]
        )
        self.network.checkpoint.assert_not_called()
        self.transport.progress.assert_not_called()
        self.assertIn(
            "resolved_at", self.journal.get("transport_error", self.route_key)
        )

    def test_full_failed_route_refund_is_a_completed_obligation(self):
        self.record.update(
            mode="refund", status="REFUNDED", delivered="0", refunded="108"
        )
        self.journal.put("quote", self.route_key, self.record)
        self.payment("refund", "failed", 108)
        self.assertTrue(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )
        self.provider.tick()
        self.network.verify_evidence.assert_not_called()
        self.transport.progress.assert_not_called()

    def test_partial_route_requires_both_confirmed_delivery_tranches(self):
        self.record["mode"] = "partial"
        self.journal.put("quote", self.route_key, self.record)
        self.payment("delivery", "first", 50)
        self.payment("delivery", "remainder", 50, confirmed=False)
        self.payment("refund", "buffer", 5)
        self.assertFalse(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )
        self.journal.settle_effect(
            f"transport:{self.route_key}:delivery:remainder",
            {"native_receipt": "remainder"},
        )
        self.assertTrue(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )

    def test_false_success_and_late_refund_keep_native_processing_active(self):
        self.payment("delivery", "first", 100)
        for mode, delivered in (("false_success", "0"), ("late_refund", "100")):
            with self.subTest(mode=mode):
                self.record.update(mode=mode, delivered=delivered, refunded="0")
                self.journal.put("quote", self.route_key, self.record)
                self.assertFalse(
                    self.provider.obligations_settled(self.route_key, self.metadata)
                )
                self.provider.tick()
        self.assertEqual(self.network.verify_evidence.call_count, 2)
        self.assertEqual(self.transport.progress.call_count, 2)

    def test_success_counters_cannot_hide_missing_or_unknown_payment_effect(self):
        self.assertFalse(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )
        self.payment("delivery", "first", 100)
        self.payment("refund", "buffer", 5)
        self.payment("refund", "unexpected", 1, confirmed=False)
        self.assertFalse(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )

    def test_changed_quote_does_not_reuse_previous_settlement(self):
        self.payment("delivery", "first", 100)
        self.payment("refund", "buffer", 5)
        changed = copy.deepcopy(self.record)
        changed["response"]["quote"]["depositAddress"] = "different-lp"
        self.journal.put("quote", self.route_key, changed)
        self.assertFalse(
            self.provider.obligations_settled(self.route_key, self.metadata)
        )
        self.provider.tick()
        recorded = self.journal.get("transport_error", self.route_key)
        self.assertNotIn("resolved_at", recorded)
        self.transport.progress.assert_not_called()

    def test_transport_diagnostic_resolves_after_verified_progress(self):
        self.journal.put(
            "transport_error",
            self.route_key,
            {"error": "temporary RPC failure", "at": 1},
        )
        self.provider.tick()
        self.network.verify_evidence.assert_called_once()
        self.transport.progress.assert_called_once()
        self.assertIn(
            "resolved_at", self.journal.get("transport_error", self.route_key)
        )


class RelayerRecoveryTests(TransportFixture):
    def setUp(self):
        super().setUp()
        self.journal = self.app_journal
        self.network = copy.copy(self.network)
        self.network.journal = self.journal
        self.journal.put(
            "route_quote", raw_id(self.route_id), self.response, immutable=True
        )
        self.plan["routes"][0].update(received=0, returned=0, dispatched=110)
        self.command_record.update(
            action=Action.DISPATCH,
            domain=raw_id(self.source),
            plan=raw_id(self.plan_id),
            state=0,
        )
        self.network.commands = Mock(return_value=[])
        self.network.call = Mock()
        self.network.observe = Mock()
        self.relayer = Relayer(self.network)

    def owner_command(self):
        command = encode_command(
            {
                "domain": self.source,
                "operation": identity(7),
                "action": Action.OWNER_WITHDRAW,
                "amount": 1,
                "recipient": self.bindings["ethereum"]["deposit_word"],
                "expires": 2**32,
            }
        )
        return {
            "operation": raw_id(identity(7)),
            "state": 0,
            "action": Action.OWNER_WITHDRAW,
            "command": hex_bytes(command),
            "digest": hex_bytes(digest(command)),
            "domain": raw_id(self.source),
            "signature": None,
            "signing": False,
            "plan": raw_id(ZERO),
        }

    def test_lost_apply_response_recovers_committed_plan_and_exact_quote(self):
        self.assertEqual(self.journal.records("quote"), [])
        self.assertEqual(
            self.journal.get("route_quote", raw_id(self.route_id)), self.response
        )
        self.assertEqual(self.journal.records("plan"), [])
        self.assertEqual(self.journal.records("route_binding"), [])
        self.relayer.recover_plans()
        binding = self.journal.get("route_binding", raw_id(self.route_id))
        self.assertEqual(binding["plan"], raw_id(self.plan_id))
        self.assertEqual(binding["quote"], self.response)
        self.assertEqual(
            (binding["source"], binding["destination"]), ("ethereum", "solana")
        )
        self.relayer.recover_plans()
        self.assertEqual(len(self.journal.records("plan")), 1)
        self.assertEqual(len(self.journal.records("route_binding")), 1)
        self.network.call.assert_not_called()

    def test_corrupt_quote_does_not_block_an_unrelated_owner_command(self):
        self.quote["quote_hash"] = raw_id(identity(999))
        owner = self.owner_command()
        self.network.commands.return_value = [owner]
        self.relayer.tick()
        self.assertIsNone(self.journal.get("route_binding", raw_id(self.route_id)))
        error = self.journal.get("plan_recovery_error", raw_id(self.plan_id))
        self.assertEqual(error["stage"], "recovery")
        self.assertIn("commitment", error["error"])
        self.network.call.assert_any_call(
            "request_signature", role="agent", wait=False, operation=owner["operation"]
        )

    def test_recovery_error_resolves_only_when_quote_recovery_succeeds(self):
        plan_id = raw_id(self.plan_id)
        committed = self.quote["quote_hash"]
        self.quote["quote_hash"] = raw_id(identity(999))
        self.journal.put(
            "plan_error",
            plan_id,
            {"error": "old advance failure", "at": 1},
        )
        self.relayer.tick()
        self.assertNotIn(
            "resolved_at", self.journal.get("plan_recovery_error", plan_id)
        )
        self.assertIn("resolved_at", self.journal.get("plan_error", plan_id))
        self.quote["quote_hash"] = committed
        self.relayer.tick()
        self.assertIn("resolved_at", self.journal.get("plan_recovery_error", plan_id))
        self.assertIn("resolved_at", self.journal.get("plan_error", plan_id))
        self.assertIn(
            "commitment", self.journal.get("plan_recovery_error", plan_id)["error"]
        )

    def test_acknowledged_command_resolves_prior_error_without_resubmission(self):
        owner = self.owner_command()
        for record in (owner, self.command_record):
            record["state"] = CommandState.SUCCEEDED
            self.journal.put(
                "relay_error", record["operation"], {"error": "lost reply", "at": 5}
            )
        self.network.commands.return_value = [owner, self.command_record]
        self.relayer.tick()
        for record in (owner, self.command_record):
            recorded = self.journal.get("relay_error", record["operation"])
            self.assertEqual(recorded["error"], "lost reply")
            self.assertIn("resolved_at", recorded)
        self.network.checkpoint.assert_not_called()
        self.network.verify_evidence.assert_not_called()
        self.assertEqual(self.journal.records("dispatch_evidence"), [])
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
            chain.execute_command.assert_not_called()
            chain.deposit_route.assert_not_called()

    def test_watch_plan_stops_after_three_coordinator_retries_without_replacement_payment(
        self,
    ):
        plan_id = raw_id(self.plan_id)
        self.network.commands.return_value = [self.command_record]
        attempts = []

        def retry(method, **args):
            self.assertEqual(method, "retry")
            self.assertEqual(args["plan"], plan_id)
            attempts.append(args)
            if len(attempts) == 3:
                self.plan["requires_attention"] = True

        self.network.call.side_effect = retry
        original_command = copy.deepcopy(self.command_record)
        original_plan = copy.deepcopy(self.plan)
        for moment in (1000, 1059, 1060, 1120, 1180, 1240, 1300):
            with patch("worker.time.time", return_value=moment):
                self.relayer.watch_plan(plan_id, self.plan)
        self.assertEqual(len(attempts), 3)
        self.assertTrue(self.plan["requires_attention"])
        self.assertEqual(self.command_record, original_command)
        self.assertEqual(self.plan["routes"], original_plan["routes"])
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
            chain.execute_command.assert_not_called()
            chain.deposit_route.assert_not_called()

    def test_watch_plan_resets_observation_interval_on_real_progress(self):
        plan_id = raw_id(self.plan_id)
        self.network.commands.return_value = [self.command_record]
        with patch("worker.time.time", return_value=1000):
            self.relayer.watch_plan(plan_id, self.plan)
        self.command_record["state"] = 1
        with patch("worker.time.time", return_value=1060):
            self.relayer.watch_plan(plan_id, self.plan)
        self.network.call.assert_not_called()
        with patch("worker.time.time", return_value=1119):
            self.relayer.watch_plan(plan_id, self.plan)
        self.network.call.assert_not_called()
        with patch("worker.time.time", return_value=1120):
            self.relayer.watch_plan(plan_id, self.plan)
        self.network.call.assert_called_once()

    def test_watch_plan_never_retries_a_terminal_plan(self):
        for status in ("Funded", "Closed"):
            with self.subTest(status=status):
                self.plan["status"] = status
                self.relayer.watch_plan(raw_id(self.plan_id), self.plan)
        self.network.call.assert_not_called()
        self.network.commands.assert_not_called()

    def test_attention_plan_does_not_sign_or_execute_existing_commands(self):
        self.plan["requires_attention"] = True
        self.command_record["state"] = 0
        self.network.config["sign_key"] = "coordinator-key"
        for signature in (None, {"already": "signed"}):
            with self.subTest(signature=signature):
                self.command_record["signature"] = signature
                self.network.commands.return_value = [self.command_record]
                with patch(
                    "worker.normalize_signature", return_value=("0x" + "00" * 64, 0)
                ):
                    self.relayer.tick()
        self.assertNotIn(
            "request_signature",
            [call.args[0] for call in self.network.call.call_args_list],
        )
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
            chain.execute_command.assert_not_called()
            chain.deposit_route.assert_not_called()

    def test_completed_route_surplus_never_submits_another_collection(self):
        self.plan["routes"][0].update(received=100, returned=110)
        self.route_view.side_effect = lambda _route, kind: {
            "exists": True,
            "received": "100" if kind == "delivery" else "110",
            "escrow_balance": "500",
        }
        self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
            chain.execute_command.assert_not_called()
            chain.deposit_route.assert_not_called()
        self.network.checkpoint.assert_not_called()
        self.network.observe.assert_not_called()

    def test_completed_route_with_unobserved_receipt_reconciles_without_collecting_surplus(
        self,
    ):
        self.route_view.side_effect = lambda _route, kind: {
            "exists": True,
            "received": "100" if kind == "delivery" else "110",
            "escrow_balance": "500",
        }
        self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
            chain.execute_command.assert_not_called()
            chain.deposit_route.assert_not_called()
        self.assertEqual(self.network.checkpoint.call_count, 2)
        self.assertEqual(self.network.observe.call_count, 2)
        self.network.checkpoint.assert_any_call(
            "solana", plan=self.plan_id, route=self.route_id, kind="delivery"
        )
        self.network.checkpoint.assert_any_call(
            "ethereum", plan=self.plan_id, route=self.route_id, kind="refund"
        )

    def test_terminal_plans_skip_advance_but_collect_late_refunds(self):
        self.plan["routes"][0]["received"] = 100
        self.route_view.side_effect = lambda _route, kind: {
            "exists": True,
            "received": "100" if kind == "delivery" else "0",
            "escrow_balance": "0" if kind == "delivery" else "10",
        }
        for status in ("Funded", "Closed"):
            with self.subTest(status=status):
                self.plan["status"] = status
                for chain in self.network.chains.values():
                    chain.collect_route.reset_mock()
                self.network.checkpoint.reset_mock()
                self.network.observe.reset_mock()
                self.relayer.tick()
                self.network.call.assert_not_called()
                self.network.chains["solana"].collect_route.assert_not_called()
                self.network.chains["ethereum"].collect_route.assert_called_once_with(
                    raw_id(self.route_id), "refund"
                )
                self.network.checkpoint.assert_called_once_with(
                    "ethereum", plan=self.plan_id, route=self.route_id, kind="refund"
                )
                self.network.observe.assert_called_once()

    def test_collection_waits_for_route_creation_and_resolves_read_errors(self):
        self.route_view.side_effect = OSError("RPC unavailable")
        self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
        key = f"ethereum:{raw_id(self.route_id)}"
        self.assertNotIn("resolved_at", self.journal.get("collection_error", key))

        self.bindings["near"] = {
            "domain": identity(3),
            "vault": "near-vault",
            "asset": "near-usdc",
        }
        for name, binding in self.bindings.items():
            chain = Chain(name, binding=binding)
            chain.bridge = Mock()
            chain.near_view = Mock(return_value=None)
            chain.collect_route = Mock()
            self.network.chains[name] = chain
        ethereum, solana, near = (
            self.network.chains[name] for name in ("ethereum", "solana", "near")
        )
        # Actual native absent shapes: EVM registration flag, Solana account
        # existence without a received counter, and NEAR's null view result.
        ethereum.bridge.return_value = {
            "registered": False,
            "received": "0",
            "escrow_balance": "500",
        }
        solana.bridge.return_value = {"route": {"exists": False, "escrow_balance": "0"}}
        for name in ("solana", "near"):
            self.plan["destination"] = raw_id(self.bindings[name]["domain"])
            self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
        for chain in self.network.chains.values():
            chain.collect_route.assert_not_called()
        self.network.checkpoint.assert_not_called()
        self.network.observe.assert_not_called()
        near.near_view.assert_called_once_with(
            "near-vault", "get_route", {"route": raw_id(self.route_id)}
        )
        for _, error in self.journal.records("collection_error"):
            self.assertEqual(error["error"], "RPC unavailable")
            self.assertIn("resolved_at", error)

        self.plan["destination"] = raw_id(self.destination)
        key = f"solana:{raw_id(self.route_id)}"
        for malformed, missing in (
            ({"exists": True, "escrow_balance": "0"}, "received"),
            ({"received": "0", "escrow_balance": "0"}, "exists"),
            ({"exists": "false"}, "boolean"),
        ):
            solana.bridge.return_value = {"route": malformed}
            self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
            error = self.journal.get("collection_error", key)
            self.assertIn(missing, error["error"])
            self.assertNotIn("resolved_at", error)
        solana.collect_route.assert_not_called()
        solana.bridge.return_value = {
            "route": {"exists": True, "received": "0", "escrow_balance": "0"}
        }
        self.relayer.collect_routes(raw_id(self.plan_id), self.plan)
        self.assertIn("resolved_at", self.journal.get("collection_error", key))


class NativePaymentRecoveryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "state.sqlite3"
        self.deposit_route = Mock(
            side_effect=PendingEvidence("submission response lost")
        )
        self.network = SimpleNamespace()
        self.network.journal = Journal(self.path)
        self.network.config = {"chains": {"ethereum": {"vault": "vault"}}}
        self.route_view = Mock(return_value={"received": "40", "escrow_balance": "59"})
        self.network.chains = {
            "ethereum": SimpleNamespace(
                deposit_route=self.deposit_route,
                route_view=self.route_view,
            )
        }
        self.provider = TransportProvider(self.network, SimpleNamespace())
        self.route = identity(1)

    def pay(self, amount=60):
        return self.provider.pay_route(
            "ethereum", self.route, "delivery", amount, "payment-1", 100
        )

    def test_unknown_payment_survives_restart_and_only_actual_cash_settles_it(self):
        with self.assertRaisesRegex(PendingEvidence, "response lost"):
            self.pay()
        self.assertEqual(self.network.journal.effect("payment-1")["state"], "Unknown")

        # Reopen persisted state as a restarted worker would. Even after the
        # attention threshold, one missing token prevents a fresh payment.
        self.network.journal = Journal(self.path)
        for _ in range(3):
            with self.assertRaisesRegex(PendingEvidence, "remains unknown"):
                self.pay()
        self.assertEqual(self.network.journal.effect("payment-1")["state"], "Attention")
        self.deposit_route.assert_called_once()

        self.route_view.return_value = {
            "received": "40",
            "escrow_balance": "60",
        }
        result = self.pay()
        self.assertTrue(result["reconciled"])
        self.assertEqual(result["total"], "100")
        self.assertEqual(self.network.journal.effect("payment-1")["state"], "Confirmed")
        self.route_view.reset_mock()
        self.assertEqual(self.pay(), result)
        self.route_view.assert_not_called()
        self.deposit_route.assert_called_once()

    def test_changed_unknown_payment_terms_are_rejected_before_recovery(self):
        with self.assertRaises(PendingEvidence):
            self.pay()
        with self.assertRaisesRegex(ValueError, "different arguments"):
            self.pay(amount=61)
        self.route_view.assert_not_called()
        self.deposit_route.assert_called_once()

    def test_failed_finalized_read_keeps_unknown_payment_without_resubmission(self):
        with self.assertRaises(PendingEvidence):
            self.pay()
        self.route_view.side_effect = OSError("finalized RPC unavailable")
        with self.assertRaisesRegex(OSError, "finalized RPC unavailable"):
            self.pay()
        self.assertEqual(self.network.journal.effect("payment-1")["state"], "Unknown")
        self.deposit_route.assert_called_once()


if __name__ == "__main__":
    unittest.main()
