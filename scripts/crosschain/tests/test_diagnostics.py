"""Diagnostic history remains inspectable after an operation recovers."""

import fcntl
import os
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
from localnet import CHAINS, LocalnetError
from main import (
    require_ready,
    serve_environment,
    service_cycle,
    start_service,
    status,
    stop_service,
)
from protocol import Action, PrivateKey, digest, encode_command, hex_bytes, identity
from providers import MpcProvider
from worker import Relayer


class DiagnosticTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.journal = Journal(Path(temporary.name) / "state.sqlite3")
        self.provider_journal = Journal(Path(temporary.name) / "provider.sqlite3")

    def test_container_restart_preserves_data_and_sigterm_stops_services_before_nodes(
        self,
    ):
        data = self.journal.path.parent / "coordination"
        data.mkdir()
        retained = data / "manifest.json"
        retained.write_text("persistent deployment and payment identities")
        environments = {}
        actions = []
        for name in CHAINS:
            directory = data.parent / name
            directory.mkdir()
            process = directory / "process.json"
            process.write_text('{"pid":123,"identity":"old namespace"}')
            environments[name] = SimpleNamespace(
                process_file=process,
                down=lambda name=name: actions.append(name),
            )
        for role in ("provider", "relayer"):
            (data / f"{role}.process.json").write_text("old process")

        def start(stopped):
            for environment in environments.values():
                self.assertFalse(environment.process_file.exists())
            self.assertFalse(list(data.glob("*.process.json")))
            self.assertEqual(
                retained.read_text(), "persistent deployment and payment identities"
            )
            actions.append("up")
            os.kill(os.getpid(), signal.SIGTERM)
            self.assertTrue(stopped.is_set())

        with (
            patch.dict(os.environ, {"APODECT_CONTAINER": "1"}),
            patch("main.DATA", data),
            patch("main.Environment", side_effect=lambda name: environments[name]),
            patch("main.up", side_effect=start),
            patch("main.stop_service", side_effect=actions.append),
        ):
            serve_environment()
        self.assertEqual(actions, ["up", "relayer", "provider", *CHAINS])
        self.assertTrue(retained.exists())

    def test_container_rejects_concurrent_owner_before_touching_records(self):
        data = self.journal.path.parent / "coordination"
        data.mkdir()
        process = data / "provider.process.json"
        process.write_text("existing owner")
        for lock, reason in (
            (".container.lock", "another container"),
            (".lock", "another local lifecycle"),
        ):
            with (
                self.subTest(lock=lock),
                (data.parent / lock).open("a") as owner,
                patch.dict(os.environ, {"APODECT_CONTAINER": "1"}),
                patch("main.DATA", data),
                patch("main.up") as start,
                patch("main.stop_service") as stop,
            ):
                fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaisesRegex(LocalnetError, reason):
                    serve_environment()
            start.assert_not_called()
            stop.assert_not_called()
            self.assertEqual(process.read_text(), "existing owner")

    def test_container_exited_child_causes_failure_and_ordered_cleanup(self):
        data = self.journal.path.parent / "coordination"
        data.mkdir()
        actions = []
        environment = Mock()
        environment.owned_process.return_value = None
        environment.down.side_effect = lambda: actions.append("node")
        stopped = Mock()
        stopped.wait.return_value = False
        with (
            patch.dict(os.environ, {"APODECT_CONTAINER": "1"}),
            patch("main.DATA", data),
            patch("main.Environment", return_value=environment),
            patch("main.threading.Event", return_value=stopped),
            patch("main.up"),
            patch("main.stop_service", side_effect=actions.append),
        ):
            with self.assertRaisesRegex(LocalnetError, "ethereum exited"):
                serve_environment()
        self.assertEqual(actions, ["relayer", "provider", "node", "node", "node"])

    def test_container_monitor_allows_service_pause_inside_lifecycle_lock(self):
        data = self.journal.path.parent / "coordination"
        data.mkdir()
        environment = Mock()
        environment.owned_process.return_value = None
        stopped = Mock()
        iterations = 0
        with (data.parent / ".lock").open("a") as operation:

            def wait(_timeout):
                nonlocal iterations
                iterations += 1
                if iterations == 1:
                    fcntl.flock(operation, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    return False
                fcntl.flock(operation, fcntl.LOCK_UN)
                return True

            stopped.wait.side_effect = wait
            with (
                patch.dict(os.environ, {"APODECT_CONTAINER": "1"}),
                patch("main.DATA", data),
                patch("main.Environment", return_value=environment),
                patch("main.threading.Event", return_value=stopped),
                patch("main.up"),
                patch("main.stop_service") as stop,
            ):
                serve_environment()
        environment.owned_process.assert_not_called()
        self.assertEqual(iterations, 2)
        self.assertEqual(
            [call.args[0] for call in stop.call_args_list], ["relayer", "provider"]
        )

    def test_container_shutdown_does_not_race_an_active_lifecycle_command(self):
        data = self.journal.path.parent / "coordination"
        data.mkdir()
        environment = Mock()
        stopped = Mock()
        with (data.parent / ".lock").open("a") as operation:

            def stop_requested(_timeout):
                fcntl.flock(operation, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return True

            stopped.wait.side_effect = stop_requested
            with (
                patch.dict(os.environ, {"APODECT_CONTAINER": "1"}),
                patch("main.DATA", data),
                patch("main.Environment", return_value=environment),
                patch("main.threading.Event", return_value=stopped),
                patch("main.time.monotonic", side_effect=[0, 46]),
                patch("main.up"),
                patch("main.stop_service") as stop,
            ):
                with self.assertRaisesRegex(
                    LocalnetError, "prevented graceful shutdown"
                ):
                    serve_environment()
        stop.assert_not_called()
        environment.down.assert_not_called()

    def test_compose_readiness_rejects_uninitialized_stopped_or_degraded_components(
        self,
    ):
        report = {
            "initialized": True,
            "nodes": {name: {"state": "running"} for name in CHAINS},
            "services": {
                role: {"state": "running"} for role in ("provider", "relayer")
            },
            "coordinator_read_error": None,
        }
        require_ready(report)
        with self.assertRaisesRegex(LocalnetError, "initialization"):
            require_ready({"initialized": False})
        for group, name, value in (
            ("nodes", "solana", "stopped"),
            ("services", "provider", "degraded"),
        ):
            with self.subTest(component=name):
                report[group][name]["state"] = value
                with self.assertRaisesRegex(LocalnetError, name):
                    require_ready(report)
                report[group][name]["state"] = "running"
        report["coordinator_read_error"] = "RPC unavailable"
        with self.assertRaisesRegex(LocalnetError, "coordinator"):
            require_ready(report)

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

    def test_reused_service_pid_requires_a_new_completed_cycle_before_readiness(self):
        self.provider_journal.put(
            "health", "provider", {"pid": 123, "last_tick": 100, "errors": {}}
        )
        self.provider_journal.put("payment", "existing", {"state": "unknown"})
        (self.journal.path.parent / "manifest.json").write_text("{}")
        net = SimpleNamespace(journal=self.journal, view=Mock())
        provider_net = SimpleNamespace(journal=self.provider_journal)
        environment = Mock()
        environment.owned_process.return_value = None
        environment.url.return_value = "http://127.0.0.1:3030"

        def spawn(*_args, **_kwargs):
            self.assertEqual(self.provider_journal.get("health", "provider"), {})
            return SimpleNamespace(pid=123)

        with (
            patch("main.DATA", self.journal.path.parent),
            patch(
                "main.network",
                side_effect=lambda role=None: (
                    provider_net if role == "provider" else net
                ),
            ),
            patch("main.Environment", return_value=environment),
            patch("main.time.time", return_value=101),
            patch("main.process_identity", return_value="new process"),
            patch("main.subprocess.Popen", side_effect=spawn),
        ):
            with patch("main.owned_process", return_value=None):
                start_service("provider")
            with patch("main.owned_process", return_value=123):
                service = status()["services"]["provider"]
                self.assertFalse(service["heartbeat_fresh"])
                self.assertEqual(service["state"], "degraded")
                with patch("main.os.getpid", return_value=123):
                    service_cycle(provider_net, "provider", [lambda: None])
                service = status()["services"]["provider"]
                self.assertTrue(service["heartbeat_fresh"])
                self.assertEqual(service["state"], "running")
        self.assertEqual(
            self.provider_journal.get("payment", "existing"), {"state": "unknown"}
        )

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
