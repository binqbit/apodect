"""Wire, provenance and durable financial-effect failure boundaries."""

import copy
import hashlib
import json
import sys
import tempfile
import threading
import time
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from journal import Journal
from protocol import (
    CHECKPOINT_FIELDS,
    SECP_ORDER,
    Action,
    PrivateKey,
    base58_decode,
    base58_encode,
    decode_checkpoint,
    decode_command,
    domain,
    encode_command,
    foreign_hash,
    foreign_payload,
    hex_bytes,
    identity,
    normalize_signature,
    sign_mpc,
)
from providers import LiquidityClient, Transport, iso_time, provider_server


class ProtocolTests(unittest.TestCase):
    def test_signature_authenticates_every_command_word(self):
        command = {
            "domain": "0x" + "11" * 32,
            "action": Action.RESERVE,
            "operation": identity(1),
            "related": identity(2),
            "amount": 60_000_000,
            "expires": 1800000000,
        }
        raw = encode_command(command)
        key = PrivateKey.from_int(1)  # Public test vector only, never a service key.
        digest = hex_bytes(hashlib.sha256(raw).digest())
        signed = sign_mpc(key, digest)
        public = hex_bytes(key.public_key.format(compressed=False)[1:])
        normalize_signature(signed, digest, public)
        for word in range(12):
            changed = bytearray(raw)
            changed[word * 32 + 31] ^= 1
            with self.assertRaises(ValueError):
                normalize_signature(
                    signed, hex_bytes(hashlib.sha256(changed).digest()), public
                )
        self.assertEqual(decode_command(raw)["amount"], 60_000_000)

    def test_high_s_and_noncanonical_word_are_rejected(self):
        key = PrivateKey.from_int(2)
        response = sign_mpc(key, "11" * 32)
        response["s"]["scalar"] = (
            (SECP_ORDER - int(response["s"]["scalar"], 16)).to_bytes(32, "big").hex()
        )
        with self.assertRaisesRegex(ValueError, "scalar"):
            normalize_signature(
                response,
                "11" * 32,
                hex_bytes(key.public_key.format(compressed=False)[1:]),
            )
        raw = bytearray(encode_command({"action": Action.RESERVE}))
        raw[32] = 1
        with self.assertRaisesRegex(ValueError, "noncanonical"):
            decode_command(raw)

    def test_domain_separates_every_identity_and_refuses_newline_alias(self):
        args = [
            "coordinator.test.near",
            "near-genesis",
            "01" * 32,
            "solana",
            "svm-genesis",
            "vault",
            "asset",
        ]
        expected = domain(*args)
        for index in (0, 1, 4, 5, 6):
            changed = args.copy()
            changed[index] += "-other"
            self.assertNotEqual(expected, domain(*changed))
        args[5] = "vault\nasset"
        with self.assertRaises(ValueError):
            domain(*args)

    def test_numeric_hex_identity_is_never_interpreted_as_decimal(self):
        self.assertEqual(identity("0" * 62 + "10"), identity(16))
        for raw in (b"\0" * 32, b"\0\0\xff", bytes(range(64))):
            self.assertEqual(base58_decode(base58_encode(raw)), raw)

    def test_evm_identity_uses_full_word_while_command_amounts_remain_u64(self):
        address = "0xabcdef0123456789abcdef0123456789abcdef01"
        self.assertEqual(identity(int(address, 16)), "0x" + "00" * 12 + address[2:])
        maximum = (1 << 256) - 1
        self.assertEqual(identity(maximum), "0x" + "ff" * 32)
        for invalid in (-1, 1 << 256):
            with self.assertRaisesRegex(ValueError, "identity integer"):
                identity(invalid)
        with self.assertRaisesRegex(ValueError, "u64"):
            encode_command({"action": Action.RESERVE, "amount": 1 << 64})

    def test_checkpoint_rejects_missing_words_and_boolean_aliases(self):
        self.assertEqual(len(CHECKPOINT_FIELDS), 39)
        with self.assertRaises(ValueError):
            decode_checkpoint(b"\0" * 1247)
        raw = bytearray(1248)
        raw[CHECKPOINT_FIELDS.index("paused") * 32 + 31] = 2
        with self.assertRaisesRegex(ValueError, "boolean"):
            decode_checkpoint(raw)

    def test_mpc_solana_borsh_matches_independent_official_layout(self):
        # near/mpc 3.18.0 types/foreign_chain.rs: V1=0, Solana=2,
        # Finalized=1, AccountState extractor=2, Svm value=6/AccountState=1.
        request = {
            "Solana": {
                "tx_id": "11" * 64,
                "finality": "Finalized",
                "extractors": [{"AccountState": {"pubkey": "22" * 32}}],
            }
        }
        values = [
            {
                "SvmExtractedValue": {
                    "AccountState": {"owner": "33" * 32, "data": "cafe"}
                }
            }
        ]
        expected = bytes.fromhex(
            "0002"
            + "11" * 64
            + "010100000002"
            + "22" * 32
            + "010000000601"
            + "33" * 32
            + "02000000cafe"
        )
        self.assertEqual(foreign_payload(request, values), expected)
        changed = copy.deepcopy(values)
        changed[0]["SvmExtractedValue"]["AccountState"]["data"] = "caff"
        self.assertNotEqual(
            foreign_hash(request, values), foreign_hash(request, changed)
        )
        request["Solana"]["finality"] = "Confirmed"
        with self.assertRaisesRegex(ValueError, "finalized"):
            foreign_payload(request, values)


class ProviderTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "state.sqlite3"
        self.journal = Journal(self.path)
        self.assets = {
            "eth": {"chain": "ethereum", "asset": "e", "deposit_address": "lp"},
            "sol": {"chain": "solana", "asset": "s", "deposit_address": "other-lp"},
        }
        self.transport = Transport(self.journal, self.assets, fee=100, refund_fee=10)
        self.request = {
            "swapType": "EXACT_OUTPUT",
            "depositType": "ORIGIN_CHAIN",
            "recipientType": "DESTINATION_CHAIN",
            "refundType": "ORIGIN_CHAIN",
            "originAsset": "eth",
            "destinationAsset": "sol",
            "amount": "10000",
            "recipient": "vault-route",
            "refundTo": "refund-route",
            "correlationId": "quote",
            "slippageTolerance": 100,
            "deadline": iso_time(time.time() + 600),
            "dry": False,
        }

    @contextmanager
    def client(self, transport=None):
        server = provider_server(
            transport or self.transport,
            ("127.0.0.1", 0),
            lambda: {"status": "ok", "localMock": True},
        )
        thread = threading.Thread(
            target=server.serve_forever,
            kwargs={"poll_interval": 0.01},
            daemon=True,
        )
        thread.start()
        try:
            yield LiquidityClient(f"http://127.0.0.1:{server.server_port}")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())

    def test_crash_before_or_after_broadcast_never_repeats_effect(self):
        intent = {"amount": "100", "recipient": "route"}
        self.assertTrue(self.journal.begin_effect("payment", intent))
        restarted = Journal(self.path)
        self.assertFalse(restarted.begin_effect("payment", intent))
        self.assertEqual(restarted.effect("payment")["state"], "Unknown")
        with self.assertRaisesRegex(ValueError, "different arguments"):
            restarted.begin_effect("payment", {**intent, "amount": "101"})
        for _ in range(3):
            restarted.note_unknown("payment", "RPC receipt unavailable")
        self.assertEqual(restarted.effect("payment")["state"], "Attention")
        self.assertFalse(restarted.begin_effect("payment", intent))
        restarted.settle_effect("payment", {"tx_hash": "native-confirmed"})
        self.assertFalse(restarted.begin_effect("payment", intent))
        self.assertEqual(restarted.unresolved(), [])

    def test_exact_output_buffer_refund_is_independent_of_success(self):
        with self.client() as client:
            quote = client.quote(self.request)
            self.assertEqual(quote["quote"]["amountIn"], "10201")
            client.fault("quote", "late_refund")
            paid = []
            self.transport.progress(
                "quote", 10201, lambda *payment: paid.append(payment)
            )
            status = client.status(quote)
            self.assertEqual(status["status"], "SUCCESS")
            self.assertEqual(status["swapDetails"]["refundedAmount"], "0")
            self.assertEqual(status["swapDetails"]["amountIn"], "10201")
            self.assertEqual(paid, [("delivery", "first", 10000, 10000)])
            client.fault("quote", "late_refund", True)
            self.transport.progress(
                "quote", 10201, lambda *payment: paid.append(payment)
            )
            self.assertEqual(paid[-1], ("refund", "buffer", 101, 101))
            self.transport.progress(
                "quote", 10201, lambda *payment: paid.append(payment)
            )
            self.assertEqual(len(paid), 2)
            status = client.status(quote)
            self.assertEqual(status["quoteResponse"], quote)
            self.assertEqual(status["swapDetails"]["refundedAmount"], "101")
            self.assertEqual(status["swapDetails"]["amountIn"], "10100")

    def test_partial_delivery_does_not_become_full_on_retry(self):
        self.transport.quote(self.request)
        self.transport.fault("quote", "partial")
        paid = []
        for _ in range(2):
            self.transport.progress(
                "quote", 10201, lambda *payment: paid.append(payment)
            )
        self.assertEqual(paid, [("delivery", "first", 5000, 5000)])
        self.assertEqual(self.transport.status("lp", "quote")["status"], "PROCESSING")
        self.transport.fault("quote", "partial", True)
        self.transport.progress("quote", 10201, lambda *payment: paid.append(payment))
        self.assertEqual(paid[1], ("delivery", "remainder", 5000, 10000))

    def test_false_success_does_not_move_tokens(self):
        with self.client() as client:
            quote = client.quote(self.request)
            client.fault("quote", "false_success")
            paid = []
            self.transport.progress(
                "quote", 10201, lambda *payment: paid.append(payment)
            )
            status = client.status(quote)
            self.assertEqual(status["status"], "SUCCESS")
            self.assertEqual(status["swapDetails"]["amountOut"], "0")
            self.assertEqual(paid, [])

    def test_refund_cannot_change_to_delivery_after_payment(self):
        self.transport.quote(self.request)
        self.transport.fault("quote", "refund")
        paid = []
        self.transport.progress("quote", 10201, lambda *payment: paid.append(payment))
        with self.assertRaisesRegex(ValueError, "settlement mode"):
            self.transport.fault("quote", "normal")
        self.transport.progress("quote", 10201, lambda *payment: paid.append(payment))
        self.assertEqual(paid, [("refund", "failed", 10191, 10191)])

    def test_unknown_payment_freezes_mode_across_provider_restart(self):
        self.transport.quote(self.request)
        self.transport.fault("quote", "partial")
        attempted = []

        def lost_response(*payment):
            attempted.append(payment)
            raise RuntimeError("native transfer response lost")

        with self.assertRaisesRegex(RuntimeError, "response lost"):
            self.transport.progress("quote", 10201, lost_response)
        restarted = Transport(Journal(self.path), self.assets, fee=100, refund_fee=10)
        for mode in ("normal", "refund"):
            with self.assertRaisesRegex(ValueError, "settlement mode"):
                restarted.fault("quote", mode)
        restarted.fault("quote", "partial", True)
        with self.assertRaisesRegex(ValueError, "paused again"):
            restarted.fault("quote", "partial", False)
        self.assertEqual(attempted, [("delivery", "first", 5000, 5000)])
        # A persisted unsupported mode must never become an ordinary delivery.
        for mode in ("delay", "unknown"):
            record = self.journal.get("quote", "quote")
            record["mode"] = mode
            self.journal.put("quote", "quote", record)
            with self.assertRaisesRegex(ValueError, "unsupported.*mode"):
                restarted.progress("quote", 10201, lambda *_: self.fail("must not pay"))

    def test_concurrent_progress_does_not_duplicate_a_native_payment(self):
        self.transport.quote(self.request)
        entered, release = threading.Event(), threading.Event()
        paid, errors = [], []

        def pay(*payment):
            paid.append(payment)
            if payment[0] == "delivery":
                entered.set()
                self.assertTrue(release.wait(5))

        def progress():
            try:
                self.transport.progress("quote", 10201, pay)
            except BaseException as error:
                errors.append(error)

        first = threading.Thread(target=progress)
        second = threading.Thread(target=progress)
        first.start()
        self.assertTrue(entered.wait(5))
        second.start()
        release.set()
        first.join(5)
        second.join(5)
        self.assertFalse(first.is_alive() or second.is_alive())
        self.assertEqual(errors, [])
        self.assertEqual(
            paid, [("delivery", "first", 10000, 10000), ("refund", "buffer", 101, 101)]
        )

    def test_concurrent_fault_cannot_replace_inflight_settlement_mode(self):
        self.transport.quote(self.request)
        self.transport.fault("quote", "partial")
        entered, release, requested = (
            threading.Event(),
            threading.Event(),
            threading.Event(),
        )
        errors = []

        def pay(*_payment):
            entered.set()
            self.assertTrue(release.wait(5))

        def fault():
            requested.set()
            try:
                self.transport.fault("quote", "refund")
            except ValueError as error:
                errors.append(str(error))

        first = threading.Thread(
            target=lambda: self.transport.progress("quote", 10201, pay)
        )
        second = threading.Thread(target=fault)
        first.start()
        self.assertTrue(entered.wait(5))
        second.start()
        self.assertTrue(requested.wait(5))
        release.set()
        first.join(5)
        second.join(5)
        self.assertFalse(first.is_alive() or second.is_alive())
        self.assertEqual(len(errors), 1)
        self.assertIn("settlement mode", errors[0])
        self.assertEqual(self.journal.get("quote", "quote")["mode"], "partial")

    def test_quote_binding_and_maximum_input_cannot_be_changed(self):
        with self.client() as client:
            quote = client.quote(self.request)
            self.assertEqual(quote["quoteRequest"], self.request)
            self.assertEqual(quote["correlationId"], "quote")
            terms = {
                "amountIn": "10201",
                "minAmountIn": "10100",
                "amountOut": "10000",
                "minAmountOut": "10000",
                "refundFee": "10",
                "withdrawFee": "0",
                "deadline": self.request["deadline"],
                "depositAddress": "lp",
                "depositMemo": "quote",
            }
            self.assertEqual({field: quote["quote"][field] for field in terms}, terms)
            self.assertEqual(client.status(quote)["quoteResponse"], quote)
            with self.assertRaises(HTTPError) as rejected:
                client.quote({**self.request, "recipient": "another-vault"})
            self.assertEqual(rejected.exception.code, 400)
            with rejected.exception as response:
                self.assertIn("reused", json.load(response)["error"])
            with self.assertRaisesRegex(ValueError, "full quoted input"):
                self.transport.progress(
                    "quote", 10100, lambda *_: self.fail("must not pay")
                )

        restarted = Transport(Journal(self.path), self.assets, fee=999, refund_fee=99)
        with self.client(restarted) as client:
            self.assertEqual(client.quote(self.request), quote)
            self.assertEqual(client.status(quote)["quoteResponse"], quote)
            other = client.quote({**self.request, "correlationId": "other-quote"})
            self.assertEqual(other["quote"]["depositAddress"], "lp")
            self.assertEqual(other["quote"]["depositMemo"], "other-quote")
            self.assertEqual(client.status(other)["quoteResponse"], other)
            self.assertEqual(client.status(quote)["quoteResponse"], quote)
            original = restarted.quote
            changed_terms = (
                ("quoteRequest", "recipient", "another-vault", "route terms"),
                ("quote", "amountIn", "0", "quoted amountIn"),
                ("quote", "minAmountIn", str(1 << 64), "quoted minAmountIn"),
                ("quote", "amountOut", 10000, "quoted amountOut"),
                ("quote", "minAmountOut", "NaN", "quoted minAmountOut"),
                ("quote", "amountIn", "10099", "exact-output terms"),
                ("quote", "minAmountIn", "9999", "exact-output terms"),
                ("quote", "amountOut", "9999", "exact-output terms"),
                ("quote", "minAmountOut", "9999", "exact-output terms"),
                (
                    "quote",
                    "deadline",
                    iso_time(time.time() + 900),
                    "exact-output terms",
                ),
                (None, "correlationId", "other-quote", "deposit identity"),
                ("quote", "depositMemo", "other-quote", "deposit identity"),
                ("quote", "depositAddress", "", "deposit identity"),
                ("quote", "depositAddress", {"address": "lp"}, "deposit identity"),
            )
            for section, field, value, error in changed_terms:
                with self.subTest(section=section, field=field, value=value):

                    def malformed(request):
                        response = copy.deepcopy(original(request))
                        target = response if section is None else response[section]
                        target[field] = value
                        return response

                    with patch.object(restarted, "quote", side_effect=malformed):
                        with self.assertRaisesRegex(ValueError, error):
                            client.quote(self.request)
            self.assertEqual(client.quote(self.request), quote)


if __name__ == "__main__":
    unittest.main()
