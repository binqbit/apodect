"""Independent RPC evidence provenance, including copied-domain native vaults."""

import base64
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chains import Chain, Network, PendingEvidence, http_json
from protocol import (
    CHECKPOINT_FIELDS,
    CHECKPOINT_IDENTITIES,
    base58_decode,
    base58_encode,
    digest,
    identity,
    integer_word,
    unhex,
)

PROGRAM = base58_encode(bytes(range(32)))
VAULT = base58_encode(bytes(range(32, 64)))
OTHER_VAULT = base58_encode(bytes(range(64, 96)))
DOMAIN = identity(40)
OWNER = identity(41)
OBSERVATION = identity(42)
FUNDING_ID = identity(43)


def checkpoint_payload():
    fields = {
        name: identity(0) if name in CHECKPOINT_IDENTITIES else 0
        for name in CHECKPOINT_FIELDS
    }
    fields.update(
        domain=DOMAIN,
        observation=OBSERVATION,
        sequence=1,
        height=100,
        timestamp=200,
        task_state=255,
        request_state=255,
    )
    return b"".join(
        unhex(fields[name], 32)
        if name in CHECKPOINT_IDENTITIES
        else integer_word(fields[name])
        for name in CHECKPOINT_FIELDS
    )


def funding_payload():
    return b"".join(
        unhex(word, 32)
        for word in (DOMAIN, FUNDING_ID, identity(0), identity(100), OWNER)
    )


def receipt_address(vault, funding=False, bump=254):
    seed = b"funding" if funding else b"checkpoint"
    receipt_id = FUNDING_ID if funding else OBSERVATION
    return digest(
        seed
        + base58_decode(vault, 32)
        + unhex(receipt_id, 32)
        + bytes([bump])
        + base58_decode(PROGRAM, 32)
        + b"ProgramDerivedAddress"
    )


class RpcFixture(Chain):
    """Synthetic finalized RPC responses; these are verifier unit tests only."""

    def __init__(self, vault=VAULT, funding=False, seed_funding=None):
        payload = funding_payload() if funding else checkpoint_payload()
        discriminator = b"account:FundingReceipt" if funding else b"account:Checkpoint"
        self.data = digest(discriminator)[:8] + payload
        self.address = receipt_address(
            vault, funding if seed_funding is None else seed_funding
        )
        self.finality = "finalized"
        self.owner = PROGRAM
        self.binding = {
            "program": PROGRAM,
            "vault": VAULT,
            "domain": DOMAIN,
            "owner_identity": OWNER,
        }
        self.request = {
            "Solana": {
                "tx_id": "11" * 64,
                "finality": "Finalized",
                "extractors": [{"AccountState": {"pubkey": self.address.hex()}}],
            }
        }

    def rpc(self, method, params):
        if method == "getSignatureStatuses":
            return {
                "value": [
                    {"confirmationStatus": self.finality, "err": None, "slot": 100}
                ]
            }
        if method == "getAccountInfo":
            if params[0] != base58_encode(self.address):
                raise AssertionError("verifier queried a different receipt address")
            return {
                "context": {"slot": 110},
                "value": {
                    "owner": self.owner,
                    "executable": False,
                    "data": [base64.b64encode(self.data).decode(), "base64"],
                },
            }
        raise AssertionError(f"unexpected RPC method {method}")

    def verify(self):
        return self.solana_evidence(self.request, self.binding)


class SolanaEvidenceTests(unittest.TestCase):
    def test_registered_vault_checkpoint_and_funding_receipts_are_accepted(self):
        for funding in (False, True):
            with self.subTest(funding=funding):
                fixture = RpcFixture(funding=funding)
                values = fixture.verify()
                actual = values[0]["SvmExtractedValue"]["AccountState"]
                self.assertEqual(actual["owner"], base58_decode(PROGRAM, 32).hex())
                self.assertEqual(actual["data"], fixture.data.hex())

    def test_other_project_cannot_reuse_registered_domain_for_any_receipt(self):
        for funding in (False, True):
            with self.subTest(funding=funding):
                # Same program, discriminator, asset-domain, payer and finalized
                # status; only the native Project that derived the PDA differs.
                fixture = RpcFixture(vault=OTHER_VAULT, funding=funding)
                with self.assertRaisesRegex(ValueError, "registered vault"):
                    fixture.verify()

    def test_funding_and_checkpoint_seed_names_cannot_be_substituted(self):
        for funding in (False, True):
            with self.subTest(funding=funding):
                fixture = RpcFixture(funding=funding, seed_funding=not funding)
                with self.assertRaisesRegex(ValueError, "registered vault"):
                    fixture.verify()

    def test_wrong_program_and_unfinalized_receipts_are_rejected(self):
        fixture = RpcFixture()
        fixture.owner = OTHER_VAULT
        with self.assertRaisesRegex(ValueError, "owner"):
            fixture.verify()
        fixture.owner = PROGRAM
        fixture.finality = "confirmed"
        with self.assertRaises(PendingEvidence):
            fixture.verify()


class EvmRpcFixture(Chain):
    """Native event ABI with synthetic RPC finality; no chain calls or signatures."""

    def __init__(self, funding=False):
        self.name = "ethereum"
        self.binding = {
            "vault": "0x" + "12" * 20,
            "domain": DOMAIN,
            "owner_identity": OWNER,
            # keccak256 of the actual compiled event signatures.
            "checkpoint_topic": "0xe02787df0d918d92f3d940f3956727b62671c59ff8398c0840db08eaea280606",
            "funding_topic": "0x45d861fefec74dba511a298fadc2f9b0173450da0a0f02d64d7881f3f983011a",
        }
        self.payload = funding_payload() if funding else checkpoint_payload()
        data = integer_word(32) + integer_word(len(self.payload)) + self.payload
        self.request = {
            "Ethereum": {
                "tx_id": "ab" * 32,
                "finality": "Finalized",
                "extractors": [{"Log": {"log_index": 3}}],
            }
        }
        self.block = {"number": "0x64", "hash": identity(91), "timestamp": "0xc8"}
        self.finalized = {"number": "0x65"}
        self.log = {
            "removed": False,
            "address": self.binding["vault"],
            "transactionHash": "0x" + self.request["Ethereum"]["tx_id"],
            "blockHash": self.block["hash"],
            "blockNumber": self.block["number"],
            "transactionIndex": "0x1",
            "logIndex": "0x3",
            "topics": [
                self.binding["funding_topic" if funding else "checkpoint_topic"],
                FUNDING_ID if funding else OBSERVATION,
                DOMAIN,
            ],
            "data": "0x" + data.hex(),
        }
        self.receipt = {
            "status": "0x1",
            "transactionHash": self.log["transactionHash"],
            "blockHash": self.block["hash"],
            "blockNumber": self.block["number"],
            "logs": [self.log],
        }

    def rpc(self, method, params):
        if method == "eth_getTransactionReceipt":
            if params != ["0x" + self.request["Ethereum"]["tx_id"]]:
                raise AssertionError("verifier queried a different transaction")
            return self.receipt
        if method == "eth_getBlockByNumber":
            if params == ["finalized", False]:
                return self.finalized
            if params == ["0x64", False]:
                return self.block
        raise AssertionError(f"unexpected RPC request {method} {params}")

    def replace_word(self, index, value):
        data = bytearray(unhex(self.log["data"]))
        data[64 + index * 32 : 96 + index * 32] = unhex(identity(value), 32)
        self.log["data"] = "0x" + data.hex()

    def verify(self):
        return self.evm_evidence(self.request, self.binding)


class EvmEvidenceTests(unittest.TestCase):
    def test_finalized_checkpoint_and_funding_preserve_exact_event_bytes(self):
        for funding in (False, True):
            with self.subTest(funding=funding):
                fixture = EvmRpcFixture(funding=funding)
                extracted = fixture.verify()[0]["EvmExtractedValue"]["Log"]
                self.assertEqual(unhex(extracted["data"])[64:], fixture.payload)
                self.assertEqual(extracted["log_index"], 3)
                self.assertEqual(extracted["block_number"], 100)
                self.assertEqual(extracted["address"], fixture.binding["vault"][2:])

    def test_missing_failed_and_not_finalized_transactions_are_not_evidence(self):
        fixture = EvmRpcFixture()
        fixture.receipt["status"] = "0x0"
        with self.assertRaisesRegex(ValueError, "failed EVM transaction"):
            fixture.verify()
        fixture.receipt = None
        with self.assertRaisesRegex(PendingEvidence, "no receipt"):
            fixture.verify()
        fixture = EvmRpcFixture()
        for finalized in (None, {"number": "0x63"}):
            with self.subTest(finalized=finalized):
                fixture.finalized = finalized
                with self.assertRaisesRegex(PendingEvidence, "not finalized"):
                    fixture.verify()
        fixture.request["Ethereum"]["finality"] = "Latest"
        with self.assertRaisesRegex(ValueError, "unfinalized EVM"):
            fixture.verify()

    def test_orphaned_receipt_cannot_use_a_newer_finalized_head(self):
        fixture = EvmRpcFixture()
        fixture.block["hash"] = identity(92)
        with self.assertRaisesRegex(ValueError, "not canonical"):
            fixture.verify()

    def test_log_must_come_from_exact_requested_transaction_and_vault(self):
        for field, value in (
            ("removed", True),
            ("address", "0x" + "34" * 20),
            ("transactionHash", identity(99)),
            ("blockHash", identity(99)),
        ):
            with self.subTest(field=field):
                fixture = EvmRpcFixture()
                fixture.log[field] = value
                with self.assertRaisesRegex(ValueError, "provenance|log identity"):
                    fixture.verify()
        fixture = EvmRpcFixture()
        fixture.receipt["logs"] *= 2
        with self.assertRaisesRegex(ValueError, "missing exact EVM log"):
            fixture.verify()
        fixture.receipt["logs"] = []
        with self.assertRaisesRegex(ValueError, "missing exact EVM log"):
            fixture.verify()

    def test_matching_emitter_does_not_replace_event_domain_or_receipt_identity(self):
        for funding in (False, True):
            for topic in range(3):
                with self.subTest(funding=funding, topic=topic):
                    fixture = EvmRpcFixture(funding=funding)
                    fixture.log["topics"][topic] = identity(99)
                    with self.assertRaisesRegex(ValueError, "event|topic"):
                        fixture.verify()
            # Matching payload and indexed topic still cannot change the registered domain.
            fixture = EvmRpcFixture(funding=funding)
            fixture.replace_word(0, 99)
            fixture.log["topics"][2] = identity(99)
            with self.assertRaisesRegex(ValueError, "topic mismatch"):
                fixture.verify()
            fixture = EvmRpcFixture(funding=funding)
            fixture.replace_word(0, 99)
            with self.assertRaisesRegex(ValueError, "domain/height mismatch"):
                fixture.verify()
            fixture = EvmRpcFixture(funding=funding)
            fixture.replace_word(1, 99)
            with self.assertRaisesRegex(ValueError, "topic mismatch"):
                fixture.verify()

    def test_checkpoint_height_and_time_must_match_the_canonical_block(self):
        for field, value in (("height", 101), ("timestamp", 201)):
            with self.subTest(field=field):
                fixture = EvmRpcFixture()
                fixture.replace_word(CHECKPOINT_FIELDS.index(field), value)
                with self.assertRaisesRegex(ValueError, "timestamp mismatch"):
                    fixture.verify()

    def test_noncanonical_event_abi_and_wrong_funding_payer_are_rejected(self):
        for data in ("0x", "0x" + "00" * 64, EvmRpcFixture().log["data"] + "00"):
            with self.subTest(data_length=len(data)):
                fixture = EvmRpcFixture()
                fixture.log["data"] = data
                with self.assertRaisesRegex(ValueError, "canonical application"):
                    fixture.verify()
        fixture = EvmRpcFixture(funding=True)
        fixture.replace_word(4, 99)
        with self.assertRaisesRegex(ValueError, "wrong funding payer"):
            fixture.verify()

    def test_supplied_operation_cannot_substitute_authenticated_native_payload(self):
        fixture = EvmRpcFixture()
        network = Network.__new__(Network)
        network.config = {"chains": {"ethereum": fixture.binding}}
        network.chains = {"ethereum": fixture}
        evidence = {
            "chain": "ethereum",
            "payload": "0x" + fixture.payload.hex(),
            "proof": {"tx_hash": fixture.log["transactionHash"], "log_index": "3"},
        }
        network.verify_evidence(evidence)
        changed = bytearray(fixture.payload)
        operation = CHECKPOINT_FIELDS.index("operation") * 32
        changed[operation : operation + 32] = integer_word(99)
        evidence["payload"] = "0x" + changed.hex()
        with self.assertRaisesRegex(ValueError, "bytes differ"):
            network.verify_evidence(evidence)


class NearViewTests(unittest.TestCase):
    def test_contract_query_error_preserves_pending_evidence_reason(self):
        chain = Chain.__new__(Chain)
        chain.name = "near"
        chain.rpc = lambda *_: {
            "error": "Smart contract panicked: DOMAIN_NOT_REGISTERED"
        }
        with self.assertRaisesRegex(PendingEvidence, "DOMAIN_NOT_REGISTERED"):
            chain.near_view(
                "coordinator.test.near", "get_domain", {"domain": "00" * 32}
            )

    def test_successful_query_decodes_contract_json(self):
        chain = Chain.__new__(Chain)
        chain.name = "near"
        chain.rpc = lambda *_: {"result": list(b'{"available":"100"}')}
        self.assertEqual(
            chain.near_view("coordinator.test.near", "get_domain"), {"available": "100"}
        )


class LocalHttpTests(unittest.TestCase):
    def test_userinfo_cannot_disguise_a_nonlocal_destination(self):
        with patch("chains.urllib.request.build_opener") as opener:
            for url in (
                "http://127.0.0.1:123@external.example/rpc",
                "http://127.0.0.1:123@127.0.0.2/rpc",
                "https://127.0.0.1:123/rpc",
                "http://127.0.0.1/rpc",
            ):
                with (
                    self.subTest(url=url),
                    self.assertRaisesRegex(ValueError, "loopback"),
                ):
                    http_json(url)
            opener.assert_not_called()

    def test_rpc_redirect_is_rejected_before_contacting_its_destination(self):
        paths = []

        class Redirect(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                paths.append(self.path)
                self.send_response(302)
                self.send_header("Location", "/redirected")
                self.send_header("Content-Length", "0")
                self.end_headers()

        server = ThreadingHTTPServer(("127.0.0.1", 0), Redirect)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with self.assertRaisesRegex(ValueError, "redirect"):
                http_json(f"http://127.0.0.1:{server.server_port}/rpc", timeout=1)
            self.assertEqual(paths, ["/rpc"])
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
