"""Diagnostic history remains inspectable after an operation recovers."""

import signal
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chains import Chain
from journal import Journal
from main import service_cycle, status, stop_service
from protocol import Action, PrivateKey, digest, encode_command, hex_bytes, identity
from providers import MpcProvider
from worker import Relayer


class DiagnosticTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.journal = Journal(Path(temporary.name) / "state.sqlite3")
        self.provider_journal = Journal(Path(temporary.name) / "provider.sqlite3")

    def test_service_cycle_reports_failure_and_clears_it_only_after_recovery(self):
        net = SimpleNamespace(journal=self.journal)

        def unavailable_rpc():
            raise OSError("node unavailable")

        def recovered_rpc():
            return None

        with patch("main.log"):
            service_cycle(net, "provider", [unavailable_rpc])
        health = self.journal.get("health", "provider")
        self.assertEqual(list(health["errors"].values()), ["OSError: node unavailable"])
        self.assertIn("last_tick", health)
        service_cycle(net, "provider", [recovered_rpc])
        self.assertEqual(self.journal.get("health", "provider")["errors"], {})

    def test_running_service_with_failed_work_is_degraded_despite_fresh_heartbeat(self):
        self.provider_journal.put(
            "health",
            "provider",
            {"pid": 123, "last_tick": 100, "errors": {"tick": "offline"}},
        )
        (self.journal.path.parent / "manifest.json").write_text("{}")
        net = SimpleNamespace(journal=self.journal, view=Mock())
        provider_net = SimpleNamespace(journal=self.provider_journal)
        environment = Mock()
        environment.owned_process.return_value = None
        environment.url.return_value = "http://127.0.0.1:13030"
        with (
            patch("main.DATA", self.journal.path.parent),
            patch(
                "main.network",
                side_effect=lambda role=None: (
                    provider_net if role == "provider" else net
                ),
            ),
            patch("main.Environment", return_value=environment),
            patch("main.owned_process", return_value=123),
            patch("main.time.time", return_value=101),
        ):
            service = status()["services"]["provider"]
        self.assertTrue(service["heartbeat_fresh"])
        self.assertEqual(service["state"], "degraded")

    def test_lost_provider_response_recovers_from_finalized_request_without_resigning(
        self,
    ):
        self.journal.put(
            "provider_request", "0", {"state": "Unverified", "error": "lost RPC"}
        )
        near = SimpleNamespace(
            near_view=Mock(
                side_effect=[{"next_request": 1}, {"answered": True, "kind": "sign"}]
            ),
            bridge=Mock(),
        )
        provider = MpcProvider.__new__(MpcProvider)
        provider.network = SimpleNamespace(
            near=near, mpc="mock.near", journal=self.journal
        )
        provider.tick()
        near.bridge.assert_not_called()
        self.assertEqual(self.journal.get("provider_request", "0")["state"], "Closed")

    def test_provider_false_response_is_not_reported_as_accepted(self):
        raw = encode_command({"action": Action.RESERVE, "expires": 100})
        pending = {
            "answered": False,
            "kind": "sign",
            "digest": hex_bytes(digest(raw)),
            "request": {"path": "test"},
        }
        near = SimpleNamespace(
            near_view=Mock(side_effect=[{"next_request": 1}, pending]),
            bridge=Mock(return_value=False),
        )
        provider = MpcProvider.__new__(MpcProvider)
        provider.sign_key = PrivateKey.from_int(1)  # Public test vector only.
        provider.network = SimpleNamespace(
            near=near,
            mpc="mock.near",
            journal=self.journal,
            binding=Mock(),
            config={"sign_path": "test"},
            commands=Mock(
                return_value=[{"command": hex_bytes(raw), "digest": pending["digest"]}]
            ),
        )
        with patch("providers.time.time", return_value=1):
            provider.tick()
        result = self.journal.get("provider_request", "0")
        self.assertEqual(result["state"], "Unverified")
        self.assertIn("did not accept", result["error"])

    def test_shutdown_signals_service_without_interrupting_native_children(self):
        with (
            patch("main.DATA", self.journal.path.parent),
            patch("main.owned_process", side_effect=[123, None]),
            patch("main.os.kill") as terminate,
            patch("main.os.killpg") as terminate_group,
        ):
            stop_service("relayer")
        terminate.assert_called_once_with(123, signal.SIGTERM)
        terminate_group.assert_not_called()

    def test_stopped_status_overrides_stale_pid_without_querying_offline_rpc(self):
        self.provider_journal.put("health", "provider", {"pid": 123, "last_tick": 100})
        net = SimpleNamespace(journal=self.journal, view=Mock())
        provider_net = SimpleNamespace(journal=self.provider_journal)
        environment = Mock()
        environment.owned_process.return_value = None
        environment.url.return_value = "http://127.0.0.1:13030"
        (self.journal.path.parent / "manifest.json").write_text("{}")
        with (
            patch("main.DATA", self.journal.path.parent),
            patch(
                "main.network",
                side_effect=lambda role=None: (
                    provider_net if role == "provider" else net
                ),
            ),
            patch("main.Environment", return_value=environment),
            patch("main.owned_process", return_value=None),
        ):
            result = status()
        self.assertIsNone(result["services"]["provider"]["pid"])
        self.assertEqual(result["services"]["provider"]["state"], "stopped")
        self.assertFalse(result["services"]["provider"]["heartbeat_fresh"])
        self.assertIsNone(result["coordinator"])
        net.view.assert_not_called()

    def test_scan_error_resolves_only_after_successful_chain_scan(self):
        bridge = Mock(side_effect=OSError("node unavailable"))
        source = Chain("solana", binding={"domain": identity(1)})
        source.bridge = bridge
        net = SimpleNamespace(
            journal=self.journal,
            config={"chains": {"solana": {"domain": identity(1)}}},
            chains={"solana": source},
            view=Mock(return_value={"free": 0, "held": 0}),
            checkpoint=Mock(),
            observe=Mock(),
        )
        relayer = Relayer(net)
        with patch("worker.time.time", return_value=20):
            relayer.scan_local_changes()
        failure = {"error": "node unavailable", "at": 20}
        self.assertEqual(self.journal.get("scan_error", "solana"), failure)
        bridge.side_effect = None
        bridge.return_value = {
            "tasks": [],
            "next_task": 0,
            "total": 0,
            "free": 0,
            "held": 0,
        }
        with patch("worker.time.time", return_value=30):
            relayer.scan_local_changes()
        self.assertEqual(
            self.journal.get("scan_error", "solana"), {**failure, "resolved_at": 30}
        )
        with patch("worker.time.time", return_value=40):
            relayer.scan_local_changes()
        self.assertEqual(
            self.journal.get("scan_error", "solana"), {**failure, "resolved_at": 30}
        )
        bridge.side_effect = OSError("node disconnected again")
        with patch("worker.time.time", return_value=50):
            relayer.scan_local_changes()
        self.assertEqual(
            self.journal.get("scan_error", "solana"),
            {"error": "node disconnected again", "at": 50},
        )
        net.checkpoint.assert_not_called()
        net.observe.assert_not_called()


if __name__ == "__main__":
    unittest.main()
