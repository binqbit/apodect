"""Exercise lifecycle ownership and persistence without installing chain toolchains."""

import importlib.util
import os
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "localnet", Path(__file__).parents[1] / "localnet.py"
)
localnet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(localnet)


class LocalnetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.environment = localnet.Environment("ethereum", self.root)
        self.environment.directory.mkdir(parents=True)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        self.ports = {"rpc": self.port}
        localnet.write_json(self.environment.settings_file, {"ports": self.ports})

    def manifest(self):
        key = self.environment.directory / "actor.key"
        key.write_text("disposable test key")
        return {
            "schema_version": 1,
            "chain": "ethereum",
            "rpc_url": self.environment.url(),
            "contracts": {"project": "0x1"},
            "asset": {"address": "0x2"},
            "accounts": {
                role: {"address": role, "key_file": str(key)}
                for role in ("owner", "authority", "agent")
            },
        }

    def child(self):
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            start_new_session=True,
        )

        def cleanup():
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)

        self.addCleanup(cleanup)
        localnet.write_json(
            self.environment.process_file,
            {
                "pid": process.pid,
                "identity": localnet.process_identity(process.pid),
            },
        )
        return process

    def test_busy_port_is_rejected_without_stopping_listener(self):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", self.port))
            sock.listen()
            with self.assertRaisesRegex(localnet.LocalnetError, "unavailable"):
                localnet.require_free_ports(self.ports)
            self.assertEqual(sock.getsockname()[1], self.port)

    def test_invalid_and_overlapping_ports_are_rejected(self):
        with patch.dict(os.environ, {"APODECT_SOLANA_RPC_PORT": "oops"}):
            with self.assertRaisesRegex(localnet.LocalnetError, "integer"):
                localnet.configured_ports("solana")
        with patch.dict(
            os.environ,
            {"APODECT_SOLANA_RPC_PORT": "8899", "APODECT_SOLANA_FAUCET_PORT": "8900"},
        ):
            with self.assertRaisesRegex(localnet.LocalnetError, "distinct"):
                localnet.configured_ports("solana")

    def test_shutdown_stops_owned_process_and_retains_data(self):
        process = self.child()
        marker = self.environment.directory / "ledger"
        marker.write_text("persisted chain")
        self.environment.down()
        process.wait(timeout=5)
        self.assertFalse(self.environment.process_file.exists())
        self.assertEqual(marker.read_text(), "persisted chain")

    def test_reused_pid_is_never_signalled(self):
        process = self.child()
        localnet.write_json(
            self.environment.process_file,
            {
                "pid": process.pid,
                "identity": "different process start time",
            },
        )
        with self.assertRaisesRegex(localnet.LocalnetError, "ownership changed"):
            self.environment.down()
        self.assertIsNone(process.poll())

    def test_manifest_rejects_external_keys_and_wrong_rpc(self):
        manifest = self.manifest()
        localnet.write_json(self.environment.manifest_file, manifest)
        self.environment.validate_manifest()
        manifest["rpc_url"] = "https://external.invalid"
        localnet.write_json(self.environment.manifest_file, manifest)
        with self.assertRaisesRegex(localnet.LocalnetError, "does not match"):
            self.environment.validate_manifest()
        manifest["rpc_url"] = self.environment.url()
        external_key = self.root / "external.key"
        external_key.write_text("external")
        manifest["accounts"]["owner"]["key_file"] = str(external_key)
        localnet.write_json(self.environment.manifest_file, manifest)
        with self.assertRaisesRegex(localnet.LocalnetError, "external owner"):
            self.environment.validate_manifest()

    def test_reset_removes_only_selected_environment(self):
        sibling = self.environment.directory.parent / "near"
        sibling.mkdir()
        (sibling / "ledger").write_text("retain")
        self.environment.reset()
        self.assertFalse(self.environment.directory.exists())
        self.assertEqual((sibling / "ledger").read_text(), "retain")

    def test_changed_ports_require_explicit_reset(self):
        with patch.object(
            localnet, "configured_ports", return_value={"rpc": self.port + 1}
        ):
            with self.assertRaisesRegex(
                localnet.LocalnetError, "port settings changed"
            ):
                self.environment.up()

    def test_cancelled_startup_does_not_prepare_or_start_a_node(self):
        stopped = threading.Event()
        stopped.set()
        with patch.object(self.environment, "adapter_command") as adapter:
            with self.assertRaisesRegex(localnet.LocalnetError, "shutdown requested"):
                self.environment.up(stopped)
        adapter.assert_not_called()
        self.assertFalse(self.environment.process_file.exists())

    def rpc_adapter(self):
        self.environment.adapter.parent.mkdir(parents=True)
        self.environment.adapter.write_text(
            f"#!{sys.executable}\n"
            "import http.server, os\n"
            "class Handler(http.server.BaseHTTPRequestHandler):\n"
            " def do_POST(self):\n"
            "  self.rfile.read(int(self.headers['Content-Length']))\n"
            "  self.send_response(200); self.end_headers()\n"
            '  self.wfile.write(b\'{"jsonrpc":"2.0","id":1,"result":"0x7a69"}\')\n'
            " def log_message(self, *args): pass\n"
            "http.server.HTTPServer(('127.0.0.1', int(os.environ['APODECT_RPC_PORT'])), Handler).serve_forever()\n"
        )
        self.environment.adapter.chmod(0o700)

    def test_process_record_write_failure_stops_new_child(self):
        self.rpc_adapter()
        children = []
        start_process = subprocess.Popen
        write_json = localnet.write_json

        def spawn(*args, **kwargs):
            process = start_process(*args, **kwargs)
            children.append(process)
            return process

        def write(path, value):
            if path == self.environment.process_file:
                raise OSError("injected process record write failure")
            return write_json(path, value)

        with (
            patch.object(localnet, "configured_ports", return_value=self.ports),
            patch.object(self.environment, "adapter_command"),
            patch.object(localnet.subprocess, "Popen", side_effect=spawn),
            patch.object(localnet, "write_json", side_effect=write),
        ):
            with self.assertRaisesRegex(OSError, "injected"):
                self.environment.up()
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())
        self.assertFalse(self.environment.process_file.exists())
        localnet.require_free_ports(self.ports)

    def test_failed_bootstrap_stops_node_and_preserves_diagnostics(self):
        self.rpc_adapter()

        def adapter(action):
            if action == "bootstrap":
                raise localnet.LocalnetError("injected bootstrap failure")

        with (
            patch.object(localnet, "configured_ports", return_value=self.ports),
            patch.object(self.environment, "adapter_command", side_effect=adapter),
        ):
            with self.assertRaisesRegex(localnet.LocalnetError, "injected"):
                self.environment.up()
        self.assertIsNone(self.environment.owned_process())
        self.assertTrue(self.environment.log_file.exists())
        self.assertTrue(self.environment.settings_file.exists())
        localnet.require_free_ports(self.ports)

    def test_repeated_up_and_restart_preserve_deployment(self):
        self.rpc_adapter()
        actions = []

        def adapter(action):
            actions.append(action)
            if action == "bootstrap":
                localnet.write_json(self.environment.manifest_file, self.manifest())

        self.addCleanup(self.environment.down)
        with (
            patch.object(localnet, "configured_ports", return_value=self.ports),
            patch.object(self.environment, "adapter_command", side_effect=adapter),
        ):
            self.environment.up()
            first_pid = self.environment.owned_process()
            self.environment.up()
            self.assertEqual(self.environment.owned_process(), first_pid)
            original_manifest = self.environment.manifest_file.read_bytes()
            self.environment.down()
            self.environment.up()
            self.assertNotEqual(self.environment.owned_process(), first_pid)
            self.assertEqual(
                self.environment.manifest_file.read_bytes(), original_manifest
            )
            self.assertEqual(actions.count("bootstrap"), 1)
            self.environment.down()


if __name__ == "__main__":
    unittest.main()
