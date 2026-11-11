"""Local discovery decisions; real RPC/agent execution is covered by native E2E."""

import copy
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chains import Chain
from journal import Journal
from protocol import ZERO, base58_encode, identity
from worker import Relayer
from workflows import request_task, wait_observation


class NearCreationFinalityTest(unittest.TestCase):
    def test_creation_waits_for_finalized_task_without_resubmitting(self):
        source = Chain("near", binding={"vault": "project.near"})
        source.bridge = Mock(return_value=0)
        source.near_view = Mock(side_effect=[None, None, {"live_request": 0}])
        net = SimpleNamespace(chains={"near": source})
        with patch("chains.time.sleep"):
            result = request_task(net, "near", 100, observe=False)
        self.assertEqual(result["task"], identity(0))
        self.assertEqual(result["request"], identity(0))
        source.bridge.assert_called_once()

        self.assertEqual(source.near_view.call_count, 3)

    def test_lost_observation_reuses_exact_native_receipt(self):
        evidence = {"chain": "ethereum", "snapshot": {"sequence": 7}}
        net = SimpleNamespace(
            config={"chains": {"ethereum": {"domain": identity(1)}}},
            view=Mock(side_effect=[{"sequence": 6}, {"sequence": 7}]),
            journal=SimpleNamespace(records=Mock(return_value=[("proof", evidence)])),
            observe=Mock(),
        )
        with (
            patch("workflows.time.sleep"),
            patch("workflows.time.monotonic", side_effect=[0, 0, 46, 46, 46, 47]),
        ):
            result = wait_observation(net, "ethereum", 7)
        self.assertEqual(result["sequence"], 7)
        net.observe.assert_called_once_with(evidence)

    def test_unconfirmed_creation_reports_identity_without_resubmitting(self):
        from chains import PendingEvidence

        source = Chain("near", binding={"vault": "project.near"})
        source.bridge = Mock(return_value=7)
        source.near_view = Mock(return_value=None)
        net = SimpleNamespace(chains={"near": source})
        with (
            patch("chains.time.monotonic", side_effect=[0, 61]),
            self.assertRaisesRegex(PendingEvidence, identity(7)),
        ):
            request_task(net, "near", 100, observe=False)
        source.bridge.assert_called_once()


class NativeReceiptBoundaryTests(unittest.TestCase):
    def test_checkpoint_preserves_near_zero_selectors_and_native_receipt(self):
        chain = Chain("near", binding={"vault": "project.near"})
        receipt = {"payload": "immutable checkpoint bytes", "project": "project.near"}
        chain.bridge = Mock(return_value=receipt)
        self.assertIs(
            chain.checkpoint(identity(7), task=ZERO, request=ZERO, plan=None), receipt
        )
        chain.bridge.assert_called_once_with(
            "checkpoint", observation=identity(7), task=0, request=0
        )

    def test_foreign_evidence_uses_exact_native_envelope_and_rejects_aliases(self):
        evm = Chain("ethereum")
        transaction = identity(12)
        self.assertEqual(
            evm.evidence_request({"tx_hash": transaction, "log_index": 3})["Ethereum"][
                "tx_id"
            ],
            transaction[2:],
        )
        with self.assertRaises(KeyError):
            evm.evidence_request({"proof": {"tx_hash": transaction, "log_index": 3}})

        solana = Chain("solana")
        signature = base58_encode(bytes(range(64)))
        account = base58_encode(bytes(range(32)))
        proof = {"tx_hash": signature, "receipt_account": account}
        request = solana.evidence_request({"proof": proof})["Solana"]
        self.assertEqual(request["tx_id"], bytes(range(64)).hex())
        self.assertEqual(
            request["extractors"][0]["AccountState"]["pubkey"], bytes(range(32)).hex()
        )
        for unsupported in (
            proof,
            {"proof": {"signature": signature, "receipt_account": account}},
            {"proof": {"tx": signature, "receipt_account": account}},
        ):
            with self.subTest(envelope=unsupported), self.assertRaises(KeyError):
                solana.evidence_request(unsupported)


class SolanaSource(Chain):
    def __init__(self, binding):
        super().__init__("solana", binding=binding)
        self.tasks = []
        self.calls = []

    def native_create(self, number):
        task, request = identity(number + 100), identity(number + 200)
        self.tasks.append(
            {
                "task": task,
                "request": request,
                "record": {
                    "state": 0,
                    "paused": False,
                    "available": "0",
                    "request": {"id": request, "amount": "100", "state": 0},
                },
            }
        )
        return task, request

    def bridge(self, action, **arguments):
        if action != "scan":
            raise AssertionError(
                "discovery must only read native scan, not create tasks"
            )
        self.calls.append(arguments)
        start = int(arguments["from"])
        end = min(start + int(arguments["limit"]), len(self.tasks))
        return {
            "next_task": str(end),
            "total": str(len(self.tasks)),
            "free": "100",
            "held": "0",
            "tasks": copy.deepcopy(self.tasks[start:end]),
        }


class NearSource(Chain):
    def __init__(self, binding):
        super().__init__("near", binding=binding)
        self.tasks = []
        self.requests = {}

    def native_create(self):
        task = len(self.tasks)
        request = len(self.requests)
        self.requests[request] = {"amount": "100", "state": "Pending", "task_id": task}
        self.tasks.append(
            {"state": "Active", "live_request": request, "available": "0"}
        )
        return task, request

    def near_view(self, vault, method, arguments=None):
        if method == "get_project":
            return {"next_task": len(self.tasks), "free": "100"}
        if method == "get_bridge":
            return {"held": "0"}
        if method == "get_task":
            return copy.deepcopy(self.tasks[arguments["task_id"]])
        if method == "get_request":
            return copy.deepcopy(self.requests[arguments["request_id"]])
        raise AssertionError(f"unexpected native view {method}")


class EthereumSource(Chain):
    def __init__(self, binding):
        super().__init__("ethereum", binding=binding)
        self.fail = False

    def bridge(self, action, **arguments):
        if self.fail:
            raise OSError("Ethereum RPC unavailable")
        if action == "view" and arguments["view"] == "project":
            return {"task_count": "0", "free": "100", "held": "0"}
        raise AssertionError("unexpected Ethereum discovery action")


class ObservedNetwork:
    def __init__(self, path):
        self.config = {
            "chains": {
                name: {"domain": identity(index + 1), "vault": f"native-{name}"}
                for index, name in enumerate(("ethereum", "solana", "near"))
            }
        }
        self.journal = Journal(path)
        self.chains = {
            "ethereum": EthereumSource(self.config["chains"]["ethereum"]),
            "solana": SolanaSource(self.config["chains"]["solana"]),
            "near": NearSource(self.config["chains"]["near"]),
        }
        self.near = self.chains["near"]
        self.proofs = []
        self.submitted = []
        self.sequences = {name: 0 for name in self.chains}
        self.observed = {}

    def checkpoint(self, name, **selectors):
        self.sequences[name] += 1
        proof = {
            "chain": name,
            "selectors": selectors,
            "snapshot": {"sequence": self.sequences[name]},
        }
        self.proofs.append(copy.deepcopy(proof))
        return proof

    def observe(self, proof):
        self.submitted.append(copy.deepcopy(proof))
        task = proof["selectors"].get("task")
        if task is not None:
            self.observed[(proof["chain"], identity(task))] = proof["snapshot"][
                "sequence"
            ]

    def view(self, method, **arguments):
        if method == "get_domain":
            return {"free": "100", "held": "0"}
        if method == "get_task":
            name = next(
                name
                for name, binding in self.config["chains"].items()
                if identity(arguments["domain"]) == binding["domain"]
            )
            sequence = self.observed.get((name, identity(arguments["task"])))
            return None if sequence is None else {"sequence": str(sequence)}
        raise AssertionError(f"unexpected coordinator view {method}")


class ScanTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.net = ObservedNetwork(Path(temporary.name) / "journal.sqlite3")
        self.relayer = Relayer(self.net)

    def test_external_solana_creation_discovered_without_cli_history(self):
        self.relayer.scan_local_changes()
        self.assertEqual(self.net.proofs, [])
        task, request = self.net.chains["solana"].native_create(0)
        self.relayer.scan_local_changes()
        self.assertEqual(
            self.net.proofs[-1]["selectors"], {"task": task, "request": request}
        )
        self.assertEqual(self.net.submitted, self.net.proofs)
        self.assertTrue(self.net.chains["solana"].calls)

    def test_observed_unchanged_fingerprint_creates_no_more_checkpoints(self):
        self.net.chains["solana"].native_create(0)
        self.relayer.scan_local_changes()
        self.relayer.scan_local_changes()
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 1)
        self.assertEqual(len(self.net.submitted), 1)

    def test_solana_task_change_and_live_request_replacement_get_new_evidence(self):
        source = self.net.chains["solana"]
        task, initial = source.native_create(0)
        self.relayer.scan_local_changes()
        source.tasks[0]["record"]["paused"] = True
        self.relayer.scan_local_changes()
        replacement = identity(300)
        source.tasks[0]["request"] = replacement
        source.tasks[0]["record"]["request"]["id"] = replacement
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 3)
        self.assertEqual(self.net.proofs[0]["selectors"]["request"], initial)
        self.assertEqual(
            self.net.proofs[-1]["selectors"], {"task": task, "request": replacement}
        )

    def test_near_zero_task_and_request_ids_survive_replacement_and_finalization(self):
        source = self.net.near
        task, request = source.native_create()
        self.assertEqual((task, request), (0, 0))
        self.relayer.scan_local_changes()
        self.assertEqual(
            self.net.proofs[-1]["selectors"], {"task": ZERO, "request": ZERO}
        )
        source.requests[1] = {"amount": "200", "state": "Pending", "task_id": 0}
        source.tasks[0]["live_request"] = 1
        self.relayer.scan_local_changes()
        self.assertEqual(
            self.net.proofs[-1]["selectors"], {"task": ZERO, "request": identity(1)}
        )
        source.tasks[0].update(state="Completed", live_request=None)
        self.relayer.scan_local_changes()
        self.assertEqual(
            self.net.proofs[-1]["selectors"], {"task": ZERO, "request": None}
        )
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 3)

    def test_failed_chain_does_not_block_discovery_on_other_chains(self):
        self.net.chains["ethereum"].fail = True
        self.net.chains["solana"].native_create(0)
        self.net.near.native_create()
        self.relayer.scan_local_changes()
        self.assertEqual(
            {proof["chain"] for proof in self.net.proofs}, {"solana", "near"}
        )
        self.assertEqual(len(self.net.submitted), 2)
        self.assertIn(
            "Ethereum RPC unavailable",
            self.net.journal.get("scan_error", "ethereum")["error"],
        )

    def test_pagination_visits_all_native_tasks_and_wraps_without_duplicate_proofs(
        self,
    ):
        source = self.net.chains["solana"]
        for index in range(12):
            source.native_create(index)
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 10)
        self.assertEqual(self.net.journal.get("scan_cursor", "solana"), 10)
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 12)
        self.assertEqual(self.net.journal.get("scan_cursor", "solana"), 0)
        self.relayer.scan_local_changes()
        self.assertEqual(len(self.net.proofs), 12)


if __name__ == "__main__":
    unittest.main()
