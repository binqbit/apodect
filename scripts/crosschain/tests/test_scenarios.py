"""Acceptance helpers distinguish concurrent observations and existing user work."""

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from main import suspend_relayer
from protocol import identity
from scenarios import check, smoke


class AcceptanceHelpersTest(unittest.TestCase):
    def test_fault_is_configured_before_dispatch_and_failed_setup_resumes_relayer(self):
        events = []
        net = SimpleNamespace(
            view=Mock(return_value={"queue_head": 0, "queue_length": 0}),
            call=Mock(),
            config={
                "provider_url": "http://127.0.0.1:18080",
                "chains": {"ethereum": {"domain": identity(1)}},
            },
        )

        def applied(*args, **kwargs):
            self.assertEqual(events, ["stop"])
            events.append("apply")
            return {"plan": identity(1), "routes": [{"route": identity(2)}]}

        def failed_fault(*args):
            self.assertEqual(events, ["stop", "apply"])
            events.append("fault")
            raise OSError("fault configuration failed")

        with (
            patch(
                "scenarios.check",
                return_value={"domains": {"ethereum": {"free": 0, "held": 0}}},
            ),
            patch(
                "scenarios.request_task",
                return_value={"task": identity(1), "request": identity(1)},
            ),
            patch("scenarios.wait_until"),
            patch("scenarios.apply_request", side_effect=applied),
            patch("providers.LiquidityClient.fault", side_effect=failed_fault),
            patch("main.stop_service", side_effect=lambda role: events.append("stop")),
            patch(
                "main.start_service", side_effect=lambda role: events.append("start")
            ),
            self.assertRaisesRegex(OSError, "fault configuration failed"),
        ):
            smoke(net, Mock(), suspend_relayer)
        self.assertEqual(events, ["stop", "apply", "fault", "start"])

    def test_busy_queue_is_rejected_before_creating_any_fixture_task(self):
        net = SimpleNamespace(
            view=Mock(return_value={"queue_head": 1, "queue_length": 2}),
            config={"provider_url": "http://127.0.0.1:18080"},
        )
        with (
            patch("scenarios.request_task") as create,
            self.assertRaisesRegex(ValueError, "idle approval queue"),
        ):
            smoke(net, Mock(), Mock())
        create.assert_not_called()

    def test_newer_accepted_observation_can_supersede_checkpoint_balances(self):
        net = SimpleNamespace(
            coordinator="coordinator.near",
            config={
                "provider_url": "http://127.0.0.1:18080",
                "chains": {
                    "near": {"vault": "vault", "asset": "usdc", "rpc_url": "rpc"}
                },
            },
            journal=SimpleNamespace(unresolved=Mock(return_value=[])),
        )
        with (
            patch(
                "providers.LiquidityClient.health",
                return_value={
                    "local_mock": True,
                    "coordinator": net.coordinator,
                    "unresolved_payments": [],
                },
            ),
            patch(
                "scenarios.synchronize",
                return_value={"snapshot": {"sequence": 4, "free": 100, "held": 0}},
            ),
            patch(
                "scenarios.wait_observation",
                return_value={
                    "sequence": 5,
                    "free": 75,
                    "held": 25,
                    "vault": "vault",
                    "asset": "usdc",
                },
            ),
        ):
            result = check(net)
        self.assertEqual(result["domains"]["near"]["checkpoint_sequence"], 4)
        self.assertEqual(result["domains"]["near"]["sequence"], 5)
        self.assertEqual(result["domains"]["near"]["free"], 75)


if __name__ == "__main__":
    unittest.main()
