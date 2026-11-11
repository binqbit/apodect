"""Payment recovery checks; native deployment coverage belongs to the local chain smoke."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from bridge import EthereumBridge
from localnet import LocalEthereum, RpcError, read_json, write_json


class PaymentRecoveryTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.local = LocalEthereum.__new__(LocalEthereum)
        self.local.directory = Path(self.temporary.name)
        self.local.accounts_path = self.local.directory / "accounts.json"
        self.local.url = "http://127.0.0.1:28545"
        self.local.rpc = Mock()
        self.local.values = Mock()
        self.local.wait_finalized = Mock()
        self.adapter = EthereumBridge.__new__(EthereumBridge)
        self.adapter.local = self.local
        self.adapter.directory = self.local.directory / "bridge"
        self.adapter.directory.mkdir()
        self.adapter.accounts = {
            "liquidity": {"key_file": "local-key", "password_file": "local-password"},
            "owner": {"address": "0x9876"},
        }
        self.adapter.token = "0xtoken"
        write_json(self.local.accounts_path, self.adapter.accounts)
        self.tx_hash = "0x" + "ab" * 32
        self.receipt = {"status": "0x1", "transactionHash": self.tx_hash}
        self.arguments = (
            "liquidity",
            "0x1234",
            "transfer(address,uint256)",
            "0x5678",
            42,
        )

    def journal(self):
        path = self.local.transaction_path("route-payment")
        write_json(
            path,
            {
                "binding": {
                    "role": "liquidity",
                    "target": "0x1234",
                    "signature": "transfer(address,uint256)",
                    "args": ["0x5678", "42"],
                },
                "raw_transaction": "0x010203",
                "tx_hash": self.tx_hash,
            },
        )
        return path

    def test_exact_retry_after_lost_response_never_signs_or_broadcasts_again(self):
        self.journal()
        self.local.rpc.return_value = self.receipt
        with patch("localnet.run") as run:
            result = self.local.transact(*self.arguments, intent_id="route-payment")
        self.assertEqual(result, self.receipt)
        run.assert_not_called()
        self.local.wait_finalized.assert_called_once_with(self.receipt)
        self.local.rpc.assert_called_once_with(
            "eth_getTransactionReceipt", [self.tx_hash]
        )

    def test_signed_transaction_is_durable_before_broadcast_and_reused_after_failure(
        self,
    ):
        path = self.local.transaction_path("route-payment")

        def fail_broadcast(method, params):
            if method == "eth_sendRawTransaction":
                record = read_json(path)
                self.assertEqual(record["raw_transaction"], params[0])
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                raise RuntimeError("Connection lost before response")
            return None

        self.local.rpc.side_effect = fail_broadcast
        with patch("localnet.run", side_effect=["0x010203", self.tx_hash]):
            with self.assertRaisesRegex(RuntimeError, "outcome unknown"):
                self.local.transact(*self.arguments, intent_id="route-payment")
        self.local.rpc.reset_mock()
        self.local.rpc.side_effect = [None, self.tx_hash, self.receipt]
        with patch("localnet.run") as run, patch("localnet.time.sleep"):
            self.assertEqual(
                self.local.transact(*self.arguments, intent_id="route-payment"),
                self.receipt,
            )
        run.assert_not_called()
        self.local.rpc.assert_any_call("eth_sendRawTransaction", ["0x010203"])

    def test_pending_journal_blocks_nonce_reuse_for_a_different_payment(self):
        self.journal()
        self.local.rpc.return_value = None
        for request in ({"transfer_id": "another-route"}, {}):
            with patch("localnet.run") as run:
                with self.assertRaisesRegex(RuntimeError, "Reconcile journaled"):
                    self.local.transact(
                        *self.arguments, intent_id=request.get("transfer_id")
                    )
            run.assert_not_called()

    def test_transfer_id_cannot_be_rebound_and_known_failure_is_not_retried(self):
        self.journal()
        with self.assertRaisesRegex(RuntimeError, "another payment"):
            self.local.transact(*self.arguments[:-1], 43, intent_id="route-payment")
        self.local.rpc.assert_not_called()
        self.local.rpc.return_value = {"status": "0x0"}
        with patch("localnet.run") as run:
            with self.assertRaisesRegex(RuntimeError, "Payment reverted"):
                self.local.transact(*self.arguments, intent_id="route-payment")
        run.assert_not_called()

    def test_funding_recovery_reads_saved_hash_without_signing_or_broadcast(self):
        request = {"transfer_id": "owner-funding", "project": "0x1234", "amount": "42"}
        write_json(
            self.local.transaction_path(request["transfer_id"]),
            {
                "binding": {
                    "role": "owner",
                    "target": "0x1234",
                    "signature": "coordinatedFund(uint256,bytes32)",
                    "args": ["42", "0x" + "00" * 32],
                },
                "raw_transaction": "0x010203",
                "tx_hash": self.tx_hash,
            },
        )
        log = {"address": "0x1234", "topics": ["funding-topic"]}
        self.local.rpc.return_value = {**self.receipt, "logs": [log]}
        with (
            patch("bridge.run", return_value="funding-topic"),
            patch.object(
                self.adapter, "funding_receipt", return_value={"payload": "recorded"}
            ),
            patch("localnet.run") as run,
        ):
            self.assertEqual(
                self.adapter.read_funding(request), {"payload": "recorded"}
            )
        run.assert_not_called()
        self.local.rpc.assert_called_once_with(
            "eth_getTransactionReceipt", [self.tx_hash]
        )
        self.local.wait_finalized.assert_called_once_with(log)
        self.local.rpc.reset_mock()
        with self.assertRaisesRegex(RuntimeError, "binding mismatch"):
            self.adapter.read_funding({**request, "amount": "43"})
        self.local.rpc.assert_not_called()

    def test_saved_funding_intent_skips_allowance_and_never_submits_another_approval(
        self,
    ):
        request = {"transfer_id": "owner-funding", "project": "0x1234", "amount": "42"}
        write_json(
            self.local.transaction_path(request["transfer_id"]),
            {
                "binding": {
                    "role": "owner",
                    "target": "0x1234",
                    "signature": "coordinatedFund(uint256,bytes32)",
                    "args": ["42", "0x" + "00" * 32],
                },
                "raw_transaction": "0x010203",
                "tx_hash": self.tx_hash,
            },
        )
        log = {"address": "0x1234", "topics": ["funding-topic"]}
        receipt = {**self.receipt, "logs": [log]}
        self.local.rpc.return_value = receipt
        with (
            patch("localnet.run") as native_run,
            patch("bridge.run", return_value="funding-topic"),
            patch.object(
                self.adapter, "funding_receipt", return_value={"payload": "recorded"}
            ),
        ):
            self.assertEqual(self.adapter.fund(request), {"payload": "recorded"})
        native_run.assert_not_called()
        self.local.values.assert_not_called()
        self.local.rpc.assert_called_once_with(
            "eth_getTransactionReceipt", [self.tx_hash]
        )
        self.local.wait_finalized.assert_called_once_with(receipt)

    def test_missing_funding_transaction_remains_unknown_without_any_broadcast(self):
        request = {"transfer_id": "owner-funding", "project": "0x1234", "amount": "42"}
        self.assertIsNone(self.adapter.read_funding(request))
        self.local.rpc.assert_not_called()
        write_json(
            self.local.transaction_path(request["transfer_id"]),
            {
                "binding": {
                    "role": "owner",
                    "target": "0x1234",
                    "signature": "coordinatedFund(uint256,bytes32)",
                    "args": ["42", "0x" + "00" * 32],
                },
                "raw_transaction": "0x010203",
                "tx_hash": self.tx_hash,
            },
        )
        self.local.rpc.return_value = None
        with patch("localnet.run") as run:
            self.assertIsNone(self.adapter.read_funding(request))
        run.assert_not_called()
        self.local.rpc.assert_called_once_with(
            "eth_getTransactionReceipt", [self.tx_hash]
        )

    def test_funding_waits_for_finalized_canonical_receipt_block(self):
        log = {"blockNumber": "0xa", "blockHash": "0xabcd"}
        self.local.rpc.side_effect = [
            {"number": "0x9"},
            {"number": "0xa"},
            {"hash": "0xabcd"},
        ]
        with patch("localnet.time.sleep") as sleep:
            LocalEthereum.wait_finalized(self.local, log)
        sleep.assert_called_once_with(0.2)
        self.local.rpc.assert_called_with("eth_getBlockByNumber", ["0xa", False])

    def test_orphaned_funding_receipt_is_never_accepted_as_final(self):
        self.local.rpc.side_effect = [{"number": "0xa"}, {"hash": "0xbbbb"}]
        with self.assertRaisesRegex(RuntimeError, "canonical finalized"):
            LocalEthereum.wait_finalized(
                self.local, {"blockNumber": "0xa", "blockHash": "0xaaaa"}
            )


class TransactionIdentityTest(unittest.TestCase):
    def setUp(self):
        self.adapter = EthereumBridge.__new__(EthereumBridge)
        self.project = "0x" + "11" * 20
        self.agent = "0x" + "22" * 20
        self.adapter.accounts = {"agent": {"address": self.agent}}
        self.adapter.local = SimpleNamespace(transact=Mock(), values=Mock())
        self.adapter.view = Mock(return_value={"live_request": "999"})
        self.receipt = {
            "transactionHash": "0x" + "ab" * 32,
            "blockHash": "0x" + "cd" * 32,
            "blockNumber": "0x10",
            "logs": [],
        }
        self.adapter.local.transact.return_value = self.receipt

    def test_created_ids_come_from_receipt_despite_concurrent_state_changes(self):
        self.receipt["logs"] = [
            {
                "address": self.project,
                "topics": ["0xevent", f"0x{7:064x}", f"0x{int(self.agent, 16):064x}"],
                "data": f"0x{12:064x}",
            }
        ]
        with patch("bridge.run", return_value="0xevent"):
            result = self.adapter.agent(
                {"project": self.project, "method": "create_task", "amount": "42"}
            )
        self.assertEqual((result["task_id"], result["request_id"]), ("7", "12"))
        self.adapter.local.values.assert_not_called()
        self.adapter.view.assert_called_once_with(
            {"project": self.project, "view": "task", "id": 7}
        )

    def test_new_budget_id_does_not_come_from_later_live_request(self):
        self.receipt["logs"] = [
            {
                "address": self.project,
                "topics": ["0xevent", f"0x{15:064x}", f"0x{7:064x}"],
                "data": "0x" + f"{42:064x}{1:064x}{1:064x}",
            }
        ]
        with patch("bridge.run", return_value="0xevent"):
            result = self.adapter.agent(
                {
                    "project": self.project,
                    "method": "request_budget",
                    "task_id": "7",
                    "amount": "42",
                    "purpose_ref": "1",
                }
            )
        self.assertEqual((result["task_id"], result["request_id"]), ("7", "15"))

    def test_foreign_emitter_or_agent_cannot_supply_created_ids(self):
        log = {
            "address": self.project,
            "topics": ["0xevent", f"0x{7:064x}", f"0x{0:064x}"],
            "data": f"0x{12:064x}",
        }
        self.receipt["logs"] = [log]
        with patch("bridge.run", return_value="0xevent"):
            with self.assertRaisesRegex(RuntimeError, "Malformed TaskCreated"):
                self.adapter.agent(
                    {"project": self.project, "method": "create_task", "amount": "42"}
                )
            log["address"] = self.agent
            with self.assertRaisesRegex(RuntimeError, "Missing or ambiguous"):
                self.adapter.agent(
                    {"project": self.project, "method": "create_task", "amount": "42"}
                )
        self.adapter.view.assert_not_called()


class BootstrapProjectTest(unittest.TestCase):
    def test_existing_native_project_is_reused_and_used_unbound_project_is_rejected(
        self,
    ):
        local = SimpleNamespace(
            values=Mock(),
            rpc=Mock(return_value="0xgenesis"),
            transact=Mock(),
            url="http://127.0.0.1:8545",
        )
        adapter = EthereumBridge.__new__(EthereumBridge)
        adapter.local = local
        adapter.manifest = {
            "contracts": {"project": "0xproject", "registry": "0xregistry"}
        }
        adapter.accounts = {"liquidity": {"address": "0xlp"}}
        adapter.token = "0xtoken"
        adapter.event_topic = Mock(return_value="0xtopic")
        for bound, tasks, accepted in (
            (False, 0, True),
            (True, 7, True),
            (False, 1, False),
        ):
            with self.subTest(bound=bound, tasks=tasks):
                local.values.side_effect = (
                    lambda target, field, *args, bound=bound, tasks=tasks: [
                        int(bound)
                        if field == "coordinationDomain()"
                        else tasks
                        if field == "taskCount()"
                        else 0
                    ]
                )
                if accepted:
                    result = adapter.bootstrap({})
                    self.assertEqual(result["project"], "0xproject")
                    self.assertEqual(result["vault"], "0xproject")
                else:
                    with self.assertRaisesRegex(RuntimeError, "already used"):
                        adapter.bootstrap({})
        local.transact.assert_not_called()


class FinalizedViewTest(unittest.TestCase):
    def test_native_views_and_node_configuration_always_advance_finality(self):
        with patch.dict(
            "os.environ", {"APODECT_LOCALNET_DIR": "/tmp/apodect-test"}, clear=True
        ):
            local = LocalEthereum()
        local.rpc = Mock(return_value=f"0x{42:064x}")
        with patch("localnet.run", return_value="0x1234"):
            self.assertEqual(local.values("0xproject", "freeBalance()"), [42])
        local.rpc.assert_called_once_with(
            "eth_call", [{"to": "0xproject", "data": "0x1234"}, "finalized"]
        )
        with (
            patch("localnet.Path.exists", return_value=True),
            patch("localnet.os.execvp") as launch,
        ):
            local.serve()
        args = launch.call_args.args[1]
        self.assertEqual(args[args.index("--block-time") + 1], "1")
        self.assertEqual(args[args.index("--slots-in-an-epoch") + 1], "1")
        self.assertIn("--mixed-mining", args)

    def test_included_payment_is_not_returned_when_finality_is_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            local = LocalEthereum.__new__(LocalEthereum)
            local.directory = Path(directory)
            local.accounts_path = local.directory / "accounts.json"
            local.url = "http://127.0.0.1:28545"
            local.wait_finalized = Mock(side_effect=RuntimeError("not finalized"))
            write_json(
                local.accounts_path,
                {"liquidity": {"key_file": "key", "password_file": "password"}},
            )
            receipt = {"status": "0x1", "transactionHash": "included"}
            with patch("localnet.run", return_value=json.dumps(receipt)) as run:
                with self.assertRaisesRegex(RuntimeError, "not finalized"):
                    local.transact(
                        "liquidity",
                        "token",
                        "transfer(address,uint256)",
                        "recipient",
                        42,
                    )
            run.assert_called_once()
            self.assertEqual(run.call_args.args[:2], ("cast", "send"))
            local.wait_finalized.assert_called_once_with(receipt)

    def test_missing_canonical_block_is_rejected_without_rebroadcast(self):
        local = SimpleNamespace(rpc=Mock(side_effect=[{"number": "0xa"}, None]))
        with self.assertRaisesRegex(RuntimeError, "canonical finalized"):
            LocalEthereum.wait_finalized(
                local, {"blockNumber": "0xa", "blockHash": "0xaaaa"}
            )


class RestoredFinalizedStateTest(unittest.TestCase):
    def setUp(self):
        self.local = LocalEthereum.__new__(LocalEthereum)
        self.unavailable = RpcError(
            "eth_call",
            {
                "code": -32602,
                "message": "BlockOutOfRangeError: block height is 1473 but requested was 1471",
            },
        )

    def test_restored_state_waits_for_readable_finalized_head_without_fallback(self):
        self.local.rpc = Mock(
            side_effect=[self.unavailable, self.unavailable, f"0x{42:064x}"]
        )
        with (
            patch("localnet.run", return_value="0x1234"),
            patch("localnet.time.sleep") as sleep,
        ):
            self.assertEqual(self.local.values("0xproject", "freeBalance()"), [42])
        self.assertEqual(sleep.call_count, 2)
        self.assertEqual(self.local.rpc.call_count, 3)
        for call in self.local.rpc.call_args_list:
            self.assertEqual(
                call.args,
                ("eth_call", [{"to": "0xproject", "data": "0x1234"}, "finalized"]),
            )

    def test_missing_finalized_state_times_out_with_original_rpc_failure(self):
        self.local.rpc = Mock(side_effect=self.unavailable)
        with (
            patch("localnet.run", return_value="0x1234"),
            patch("localnet.time.monotonic", side_effect=[0, 0, 30]),
            patch("localnet.time.sleep") as sleep,
            self.assertRaises(RpcError) as error,
        ):
            self.local.values("0xproject", "freeBalance()")
        self.assertIs(error.exception, self.unavailable)
        self.assertEqual(self.local.rpc.call_count, 2)
        sleep.assert_called_once_with(0.2)

    def test_unrelated_rpc_and_transport_errors_are_never_retried(self):
        errors = (
            RpcError("eth_call", {"code": -32000, "message": self.unavailable.message}),
            RpcError("eth_call", {"code": -32602, "message": "invalid calldata"}),
            RpcError("eth_call", {"code": 3, "message": "execution reverted"}),
            OSError("RPC disconnected"),
        )
        for original in errors:
            with (
                self.subTest(error=original),
                patch("localnet.run", return_value="0x1234"),
                patch("localnet.time.sleep") as sleep,
            ):
                self.local.rpc = Mock(side_effect=original)
                with self.assertRaises(type(original)) as raised:
                    self.local.values("0xproject", "freeBalance()")
                self.assertIs(raised.exception, original)
                self.local.rpc.assert_called_once()
                sleep.assert_not_called()


if __name__ == "__main__":
    unittest.main()
