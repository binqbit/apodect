"""Funding survives lost native/observation responses without another token send."""

import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chains import Chain, Network, PendingEvidence
from journal import Journal
from protocol import ZERO, decode_funding, digest, hex_bytes, identity
from worker import Relayer
from workflows import fund


class FundingRecoveryTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.net = Network.__new__(Network)
        self.net.journal = Journal(Path(temporary.name) / "state.sqlite3")
        self.net.config = {
            "chains": {
                name: {
                    "domain": identity(index),
                    "vault": f"{name}-vault",
                    "owner_identity": identity(index + 10),
                }
                for index, name in enumerate(("ethereum", "solana", "near"), start=1)
            }
        }
        self.net.chains = {
            name: Chain(name, binding=binding)
            for name, binding in self.net.config["chains"].items()
        }
        for chain in self.net.chains.values():
            chain.bridge = Mock()
        self.net.verify_evidence = Mock(return_value=({"request": True}, []))
        self.seen = set()
        self.net.view = Mock(
            side_effect=lambda method, **args: (
                (args["domain"], args["funding_id"]) in self.seen
                if method == "get_funding_seen"
                else None
            )
        )

        def acknowledge(method, **args):
            self.assertIn(method, ("observe_funding_near", "observe_funding_foreign"))
            funding_id = args.get("funding_id")
            if funding_id is None:
                funding_id = decode_funding("0x" + args["payload"])["funding_id"][2:]
            self.seen.add((args["domain"], funding_id))
            return {"submitted": True}

        self.net.call = Mock(side_effect=acknowledge)
        self.stderr = io.StringIO()
        self.addCleanup(patch.stopall)
        patch(
            "workflows.synchronize", return_value={"snapshot": {"sequence": 1}}
        ).start()
        patch("workflows.wait_observation").start()

    def receipt(self, name="ethereum", intent_id="payment-one", **changes):
        binding = self.net.config["chains"][name]
        native_id = hex_bytes(digest(f"funding:{intent_id}".encode()))
        values = {
            "domain": binding["domain"],
            "funding_id": native_id if name == "solana" else identity(1),
            "invoice_id": ZERO,
            "amount": 100,
            "payer": binding["owner_identity"],
        }
        values.update(changes)
        payload = "0x" + "".join(
            identity(values[field])[2:]
            for field in ("domain", "funding_id", "invoice_id", "amount", "payer")
        )
        return {
            "payload": payload,
            "funding_id": values["funding_id"],
            "funding_ref": native_id,
            "native_proof": "immutable",
        }

    def invoke(self, name="ethereum", amount=100, invoice=0, intent_id="payment-one"):
        with redirect_stderr(self.stderr):
            return fund(self.net, name, amount, invoice, intent_id)

    def start_unknown(self, name="ethereum", intent_id="payment-one"):
        self.net.chains[name].bridge.side_effect = TimeoutError("lost native response")
        with self.assertRaisesRegex(PendingEvidence, "receipt reconciliation"):
            self.invoke(name=name, intent_id=intent_id)
        self.net.chains[name].bridge.side_effect = None

    def test_intent_is_durable_and_printed_before_send_and_repeat_only_reconciles(self):
        receipt = self.receipt()

        def transfer(action, **args):
            self.assertEqual(action, "fund")
            self.assertIn("payment-one", self.stderr.getvalue())
            self.assertEqual(args["transfer_id"], "funding:payment-one")
            effect = self.net.journal.effect(args["transfer_id"])
            self.assertEqual(effect["state"], "Unknown")
            self.assertEqual(effect["request"]["amount"], "100")
            return receipt

        self.net.chains["ethereum"].bridge.side_effect = transfer
        first = self.invoke()
        second = self.invoke()
        self.assertEqual(first, second)
        self.assertTrue(first["acknowledged"])
        self.net.chains["ethereum"].bridge.assert_called_once()
        self.assertEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )

    def test_unknown_native_payment_is_not_resent_and_recovers_after_journal_reopen(
        self,
    ):
        self.start_unknown()
        bridge = self.net.chains["ethereum"].bridge
        bridge.return_value = None
        with self.assertRaisesRegex(PendingEvidence, "will not be repeated"):
            self.invoke()
        self.assertEqual(
            [call.args[0] for call in bridge.call_args_list], ["fund", "read_funding"]
        )
        self.net.journal = Journal(self.net.journal.path)
        bridge.return_value = self.receipt()
        Relayer(self.net).reconcile_funding()
        Relayer(self.net).reconcile_funding()
        self.assertEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )
        self.assertEqual(
            [call.args[0] for call in bridge.call_args_list],
            ["fund", "read_funding", "read_funding"],
        )
        state = self.net.journal.get("funding_observation", f"ethereum:{identity(1)}")
        self.assertEqual(state["state"], "Confirmed")

    def test_reusing_intent_cannot_change_chain_amount_or_invoice(self):
        self.start_unknown()
        for arguments in ({"name": "solana"}, {"amount": 101}, {"invoice": 1}):
            with (
                self.subTest(arguments=arguments),
                self.assertRaisesRegex(ValueError, "immutable"),
            ):
                self.invoke(**arguments)
        self.net.chains["ethereum"].bridge.assert_called_once()
        self.net.chains["solana"].bridge.assert_not_called()

    def test_recovered_receipt_must_match_every_financial_intent_field(self):
        self.start_unknown()
        for changes in (
            {"domain": identity(99)},
            {"amount": 101},
            {"invoice_id": identity(7)},
            {"payer": identity(88)},
        ):
            with self.subTest(changes=changes):
                self.net.chains["ethereum"].bridge.return_value = self.receipt(
                    **changes
                )
                Relayer(self.net).reconcile_funding()
                self.assertNotEqual(
                    self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
                )
                self.assertEqual(self.net.journal.records("funding"), [])
        self.net.call.assert_not_called()

    def test_solana_and_near_recover_by_stable_native_reference(self):
        for name in ("solana", "near"):
            with self.subTest(name=name):
                self.start_unknown(name=name, intent_id=name)
                bridge = self.net.chains[name].bridge
                bridge.return_value = self.receipt(name=name, intent_id=name)
                Relayer(self.net).reconcile_funding(name)
                arguments = bridge.call_args.kwargs
                self.assertEqual(bridge.call_args.args[0], "read_funding")
                self.assertEqual(
                    arguments["funding_ref" if name == "near" else "funding_id"],
                    hex_bytes(digest(f"funding:{name}".encode())),
                )
                self.assertEqual(
                    self.net.journal.effect(f"funding:{name}")["state"], "Confirmed"
                )

    def test_wrong_solana_receipt_id_does_not_settle_an_intent(self):
        self.start_unknown(name="solana")
        self.net.chains["solana"].bridge.return_value = self.receipt(
            name="solana", funding_id=identity(5)
        )
        Relayer(self.net).reconcile_funding()
        self.assertNotEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )
        self.net.call.assert_not_called()

    def test_lost_coordinator_response_recovers_acknowledgement_without_second_transfer(
        self,
    ):
        self.start_unknown()
        self.net.chains["ethereum"].bridge.return_value = self.receipt()
        acknowledge = self.net.call.side_effect

        def lose_response(method, **args):
            acknowledge(method, **args)
            raise TimeoutError("coordinator response lost")

        self.net.call.side_effect = lose_response
        Relayer(self.net).reconcile_funding()
        self.assertEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )
        Relayer(self.net).reconcile_funding()
        self.net.call.assert_called_once()
        state = self.net.journal.get("funding_observation", f"ethereum:{identity(1)}")
        self.assertEqual(state["state"], "Confirmed")
        self.assertEqual(
            [
                call.args[0]
                for call in self.net.chains["ethereum"].bridge.call_args_list
            ],
            ["fund", "read_funding"],
        )

    def test_unsubmitted_observation_retries_after_delay_using_same_immutable_receipt(
        self,
    ):
        self.start_unknown()
        self.net.chains["ethereum"].bridge.return_value = self.receipt()
        self.net.call.side_effect = TimeoutError("coordinator unavailable")
        with patch("worker.time.time", return_value=1000):
            Relayer(self.net).reconcile_funding()
        with patch("worker.time.time", return_value=1020):
            Relayer(self.net).reconcile_funding()
        self.net.call.assert_called_once()
        with patch("worker.time.time", return_value=1046):
            Relayer(self.net).reconcile_funding()
        self.assertEqual(self.net.call.call_count, 2)
        self.assertEqual(
            self.net.call.call_args_list[0], self.net.call.call_args_list[1]
        )
        self.assertEqual(self.net.chains["ethereum"].bridge.call_count, 2)

    def test_receipt_without_finalized_evidence_waits_and_does_not_settle(self):
        self.start_unknown()
        self.net.chains["ethereum"].bridge.return_value = self.receipt()
        self.net.verify_evidence.side_effect = PendingEvidence("not finalized")
        Relayer(self.net).reconcile_funding()
        self.assertNotEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )
        self.assertEqual(self.net.journal.records("funding"), [])
        self.net.call.assert_not_called()
        self.net.verify_evidence.side_effect = None
        Relayer(self.net).reconcile_funding()
        self.assertEqual(
            self.net.journal.effect("funding:payment-one")["state"], "Confirmed"
        )

    def test_known_coordinator_receipt_does_not_bypass_native_validation(self):
        receipt = self.receipt()
        self.seen.add((identity(1)[2:], identity(1)[2:]))
        self.net.verify_evidence.side_effect = ValueError("forged receipt")
        with self.assertRaisesRegex(ValueError, "forged"):
            self.net.observe_funding("ethereum", receipt)
        self.assertEqual(self.net.journal.records("funding_observation"), [])
        self.net.call.assert_not_called()

    def test_unclosed_or_missing_invoice_is_rejected_before_token_transfer(self):
        for invoice_record in (None, {}, {"closed": False}):
            with self.subTest(invoice=invoice_record):
                self.net.view.return_value = invoice_record
                self.net.view.side_effect = None
                with self.assertRaisesRegex(ValueError, "closed before linked funding"):
                    self.invoke(invoice=1)
                self.assertIsNone(self.net.journal.effect("funding:payment-one"))
        self.net.chains["ethereum"].bridge.assert_not_called()


if __name__ == "__main__":
    unittest.main()
