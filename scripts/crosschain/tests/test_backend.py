"""Application containers receive deployment data and dedicated gas keys only."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from backend import export_runtime
from localnet import LocalnetError
from main import require_ready, start_service, up


class BackendRuntimeTests(unittest.TestCase):
    def test_export_contains_only_dedicated_gas_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            keys = {}
            credentials = {
                "ethereum": {"address": "relay-ethereum", "private_key": "11" * 32},
                "solana": list(range(64)),
                "near": {"account_id": "relay-near", "private_key": "test-relay-key"},
            }
            for name, value in credentials.items():
                keys[name] = root / f"{name}.json"
                keys[name].write_text(json.dumps(value))
            chains = {}
            for name in credentials:
                chain = Mock()
                chain.bridge.return_value = {
                    "address": f"relay-{name}",
                    "key_file": str(keys[name]),
                }
                chain.environment.ports.return_value = {
                    "rpc": {"ethereum": 28545, "solana": 28899, "near": 23030}[name]
                }
                chains[name] = chain
            chains["ethereum"].rpc.side_effect = ["0x7a69", {"hash": "evm-genesis"}]
            ethereum_manifest = root / "ethereum-manifest.json"
            ethereum_manifest.write_text(json.dumps({"project_start_block": 42}))
            chains["ethereum"].environment.manifest_file = ethereum_manifest
            chains["solana"].rpc.return_value = "solana-genesis"
            genesis = {
                "chain_id": "test",
                "genesis_time": "time",
                "genesis_height": 0,
                "validators": [],
            }
            chains["near"].rpc.return_value = genesis
            network = SimpleNamespace(
                chains=chains,
                near=chains["near"],
                coordinator="coordinator.test.near",
                config={
                    "project_id": "project",
                    "provider_port": 18080,
                    "chains": {
                        name: {
                            "domain": name,
                            "vault": f"vault-{name}",
                            "asset": f"token-{name}",
                            "asset_id": f"asset-{name}",
                            "owner": f"owner-{name}",
                            "private_owner_key": "MUST_NOT_BE_EXPORTED",
                        }
                        for name in chains
                    },
                },
            )
            runtime = root / "runtime"
            with patch.dict(
                os.environ,
                {"APODECT_BACKEND_RUNTIME": str(runtime), "APODECT_CONTAINER": "1"},
            ):
                export_runtime(network)
            self.assertEqual(
                {path.name for path in runtime.iterdir()},
                {"deployment.json", "ethereum.key", "solana.json", "near.json"},
            )
            configuration = (runtime / "deployment.json").read_text()
            self.assertNotIn("MUST_NOT_BE_EXPORTED", configuration)
            self.assertNotIn("private_key", configuration)
            self.assertEqual(
                json.loads(configuration)["ethereum"]["rpc_url"],
                "http://localnet:28545",
            )
            self.assertEqual(
                json.loads(configuration)["projects"][0]["ethereum_start_block"], 42
            )
            self.assertEqual((runtime / "ethereum.key").read_text(), "11" * 32 + "\n")
            for path in runtime.iterdir():
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            for chain in chains.values():
                chain.bridge.assert_called_once_with("relay_account")

    def test_external_startup_supervises_provider_without_python_relayer(self):
        with tempfile.TemporaryDirectory() as temporary:
            entrypoint = Path(__file__).resolve().parents[1] / "main.py"
            result = subprocess.run(
                [sys.executable, "-E", str(entrypoint), "--help"],
                cwd=temporary,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("usage:", result.stdout)

            network = SimpleNamespace(
                coordinator="coordinator",
                config={"provider_url": "http://127.0.0.1:18080"},
            )
            with (
                patch.dict(os.environ, {"APODECT_RELAYER": "external"}),
                patch("main.DATA", Path(temporary)),
                patch("main.Environment"),
                patch("main.bootstrap"),
                patch("main.network", return_value=network),
                patch("main.LiquidityClient") as provider,
                patch(
                    "main.export_runtime",
                    return_value={"configuration": "deployment.json"},
                ),
                patch("main.start_service") as start,
                patch("main.synchronize") as synchronize,
                patch("builtins.print"),
            ):
                provider.return_value.health.return_value = {
                    "local_mock": True,
                    "coordinator": "coordinator",
                }
                up()
                start.assert_called_once_with("provider")
                synchronize.assert_not_called()
                require_ready(
                    {
                        "initialized": True,
                        "nodes": {
                            name: {"state": "running"}
                            for name in ("ethereum", "solana", "near")
                        },
                        "services": {
                            "provider": {"state": "running"},
                            "relayer": {"state": "external"},
                        },
                        "coordinator_read_error": None,
                    }
                )
            with patch.dict(os.environ, {"APODECT_RELAYER": "external"}):
                with self.assertRaisesRegex(LocalnetError, "Rust worker"):
                    start_service("relayer")
