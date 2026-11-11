#!/usr/bin/env python3
"""Run the three-chain Project environment and its local provider simulations."""

import argparse
import collections
import fcntl
import json
import os
import secrets
import shutil
import signal
import subprocess
import sys
import threading
import time
from contextlib import contextmanager
from pathlib import Path

from chains import Chain, Network, PendingEvidence
from journal import Journal
from localnet import (
    CHAINS,
    ROOT,
    Environment,
    LocalnetError,
    check_startup_cancelled,
    process_identity,
    read_json,
    write_json,
)
from protocol import (
    ZERO,
    base58_decode,
    canonical_json,
    digest,
    domain,
    hex_bytes,
    identity,
    private_key,
    raw_id,
)
from providers import (
    LiquidityClient,
    MpcProvider,
    Transport,
    TransportProvider,
    iso_time,
    provider_server,
)
from worker import Relayer
from workflows import (
    agent_action,
    apply_request,
    fund,
    request_task,
    synchronize,
    wait_observation,
)

DATA = ROOT / ".localnet" / "coordination"
PORT = int(os.environ.get("APODECT_PROVIDER_PORT", "18080"))


def state():
    configuration = read_json(DATA / "manifest.json")
    if configuration.get("schema_version") != 2:
        raise ValueError(
            "unsupported local environment manifest; "
            "retain it for inspection and initialize a fresh environment"
        )
    return configuration


def network(role="relayer"):
    filename = {"relayer": "state.sqlite3", "provider": "provider.sqlite3"}[role]
    return Network(state(), Journal(DATA / filename))


def log(message):
    print(f"{iso_time(time.time())} {message}", flush=True)


def bootstrap():
    """Preserve stable Project/provider identities across interrupted starts."""
    if (DATA / "manifest.json").exists():
        state()
    DATA.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = DATA / "identity.json"
    if not metadata.exists():
        write_json(metadata, {"project_id": secrets.token_hex(32)})
    project_id = read_json(metadata)["project_id"]
    sign = private_key(DATA / "keys/sign.bin", create=True)
    read = private_key(DATA / "keys/read.bin", create=True)
    sign_key = sign.public_key.format(compressed=False)[1:].hex()
    read_key = read.public_key.format(compressed=False)[1:].hex()
    chains = {name: Chain(name) for name in CHAINS}
    near = chains["near"].bridge(
        "bootstrap", project_id=project_id, sign_key=sign_key, read_key=read_key
    )
    evm = chains["ethereum"].bridge("bootstrap")
    solana = chains["solana"].bridge("bootstrap")
    coordinator = near["coordinator_id"]
    near_genesis = hex_bytes(digest(canonical_json(near["genesis"])))
    bindings = {
        "ethereum": {
            "vault": evm["vault"],
            "asset": evm["asset"]["address"],
            "owner": evm["accounts"]["owner"]["address"],
            "owner_recipient": identity(int(evm["accounts"]["owner"]["address"], 16)),
            "owner_identity": identity(int(evm["accounts"]["owner"]["address"], 16)),
            "genesis": evm["genesis_identity"],
            "deposit_address": evm["deposit_receiver"],
            "deposit_word": identity(int(evm["deposit_receiver"], 16)),
            "checkpoint_topic": evm["checkpoint_topic"],
            "funding_topic": evm["funding_topic"],
        },
        "solana": {
            "vault": solana["vault_address"],
            "asset": solana["asset"],
            "owner": solana["owner"],
            "owner_recipient": hex_bytes(
                base58_decode(solana["manifest"]["asset"]["owner_token"], 32)
            ),
            "owner_identity": hex_bytes(base58_decode(solana["owner"], 32)),
            "genesis": solana["genesis"],
            "deposit_address": solana["mock_lp"]["token_account"],
            "deposit_word": solana["mock_lp"]["recipient"],
            "program": solana["manifest"]["contracts"]["program"],
        },
        "near": {
            "vault": near["project"],
            "asset": near["asset"],
            "owner": near["owner"],
            "owner_recipient": hex_bytes(digest(near["owner"].encode())),
            "owner_identity": hex_bytes(digest(near["owner"].encode())),
            "genesis": near_genesis,
            "deposit_address": near["lp"],
            "deposit_word": hex_bytes(digest(near["lp"].encode())),
        },
    }
    for name, binding in bindings.items():
        binding["domain"] = domain(
            coordinator,
            near_genesis,
            project_id,
            name,
            binding["genesis"],
            binding["vault"],
            binding["asset"],
        )
        binding["asset_id"] = f"local:{name}:{binding['asset']}"
        binding["rpc_url"] = chains[name].environment.url()
    configuration = {
        "schema_version": 2,
        "project_id": project_id,
        "coordinator": coordinator,
        "mpc": near["mpc_id"],
        "sign_key": sign_key,
        "read_key": read_key,
        "sign_path": near["sign_path"],
        "chains": bindings,
        "provider_url": f"http://127.0.0.1:{PORT}",
        "provider_port": PORT,
    }
    path = DATA / "manifest.json"
    if path.exists() and read_json(path) != configuration:
        raise ValueError(
            "existing coordination manifest differs; no identities were overwritten"
        )
    write_json(path, configuration)
    net = network()
    coordinator_state = net.view("get_state")
    for field in ("project_id", "sign_key", "read_key", "sign_path"):
        if coordinator_state[field] != configuration[field]:
            raise ValueError(f"coordinator {field} differs from the pinned environment")
    if coordinator_state["mpc_id"] != configuration["mpc"]:
        raise ValueError("coordinator MPC account differs from the pinned environment")
    for name, binding in bindings.items():
        if name == "ethereum":
            chains[name].bridge(
                "bind",
                project=binding["vault"],
                domain=binding["domain"],
                public_key="0x" + sign_key,
            )
        elif name == "solana":
            current = chains[name].bridge("view")
            if current["domain"] == ZERO:
                chains[name].bridge(
                    "bind", domain=binding["domain"], public_key="0x" + sign_key
                )
            elif current["domain"] != binding["domain"]:
                raise ValueError("Solana Project belongs to another coordinator")
        else:
            current = chains[name].near_view(binding["vault"], "get_bridge")
            if current["domain"] is None:
                chains[name].bridge("bind", domain=raw_id(binding["domain"]))
            elif raw_id(current["domain"]) != raw_id(binding["domain"]):
                raise ValueError("NEAR Project belongs to another coordinator")
        try:
            registered = net.view("get_domain", domain=raw_id(binding["domain"]))
        except PendingEvidence as error:
            if "DOMAIN_NOT_REGISTERED" not in str(error):
                raise
            registered = None
        if registered is None:
            net.call(
                "register_domain",
                role="owner",
                domain=raw_id(binding["domain"]),
                chain=name,
                vault=binding["vault"],
                asset=binding["asset"],
                owner_recipient=raw_id(binding["owner_recipient"]),
                owner_identity=raw_id(binding["owner_identity"]),
                program=base58_decode(binding["program"], 32).hex()
                if name == "solana"
                else None,
            )
        else:
            expected = {
                "chain": name,
                "vault": binding["vault"],
                "asset": binding["asset"],
                "owner_recipient": raw_id(binding["owner_recipient"]),
                "owner_identity": raw_id(binding["owner_identity"]),
                "program": base58_decode(binding["program"], 32).hex()
                if name == "solana"
                else None,
            }
            if any(registered[field] != value for field, value in expected.items()):
                raise ValueError(
                    f"registered {name} domain differs from the pinned environment"
                )
    return configuration


def owned_process(role):
    path = DATA / f"{role}.process.json"
    if not path.exists():
        return None
    record = read_json(path)
    identity_now = process_identity(record["pid"])
    if identity_now is None:
        return None
    try:
        group = os.getpgid(record["pid"])
    except ProcessLookupError:
        return None
    if identity_now != record["identity"] or group != record["pid"]:
        raise ValueError(f"{role} process ownership changed; refusing to signal")
    return record["pid"]


def start_service(role):
    if owned_process(role):
        return
    # A new PID namespace can reuse the previous PID. Readiness belongs to a
    # completed cycle of this process, never to a retained heartbeat.
    network(role).journal.put("health", role, {})
    with (DATA / f"{role}.log").open("ab") as output:
        process = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), f"serve-{role}"],
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
    try:
        current = process_identity(process.pid)
        if current is None:
            raise ValueError(f"{role} exited at startup")
        write_json(
            DATA / f"{role}.process.json", {"pid": process.pid, "identity": current}
        )
    except BaseException:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=30)
        raise


def stop_service(role):
    pid = owned_process(role)
    if pid is not None:
        # The service handles SIGTERM after its current reconciliation pass.
        # Let its native child finish signing/submission; signalling the whole
        # group could kill it between durable intent and broadcast.
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 45
        while owned_process(role) is not None:
            if time.monotonic() >= deadline:
                raise ValueError(f"{role} did not stop gracefully; data retained")
            time.sleep(0.2)
    (DATA / f"{role}.process.json").unlink(missing_ok=True)


def service_cycle(net, role, work):
    """Record completed work separately from process liveness and RPC readiness."""
    errors = {}
    for task in work:
        try:
            task()
        except Exception as error:
            detail = f"{type(error).__name__}: {error}"
            errors[task.__qualname__] = detail
            log(f"{role}: {detail}")
    net.journal.put(
        "health",
        role,
        {"last_tick": time.time(), "pid": os.getpid(), "errors": errors},
    )


def serve(role):
    net = network(role)
    stopped = threading.Event()
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, lambda _signal, _frame: stopped.set())
    server = None
    if role == "provider":
        assets = {
            binding["asset_id"]: {"chain": name, **binding}
            for name, binding in net.config["chains"].items()
        }
        mock_transport = Transport(net.journal, assets)
        mpc = MpcProvider(net, DATA / "keys")
        delivery = TransportProvider(net, mock_transport)
        server = provider_server(
            mock_transport,
            ("127.0.0.1", net.config["provider_port"]),
            lambda: {
                "local_mock": True,
                "coordinator": net.coordinator,
                "unresolved_payments": net.journal.unresolved(),
            },
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        work = [mpc.tick, delivery.tick]
    else:
        work = [Relayer(net).tick]
    log(f"{role} started for {net.coordinator}")
    try:
        while not stopped.is_set():
            service_cycle(net, role, work)
            stopped.wait(0.5)
    finally:
        if server:
            server.shutdown()
            server.server_close()
        log(f"{role} stopped; pending effects retained")


def up(stop_requested=None):
    if (DATA / "manifest.json").exists():
        state()
    for name in CHAINS:
        Environment(name).up(stop_requested)
    check_startup_cancelled(stop_requested)
    bootstrap()
    check_startup_cancelled(stop_requested)
    start_service("provider")
    net = network()
    for _ in range(60):
        check_startup_cancelled(stop_requested)
        try:
            health = LiquidityClient(net.config["provider_url"]).health()
            if (
                health.get("local_mock") is not True
                or health.get("coordinator") != net.coordinator
            ):
                raise ValueError("provider endpoint belongs to another environment")
            break
        except OSError:
            time.sleep(0.5)
    else:
        raise ValueError("local provider HTTP service did not start")
    for name in net.chains:
        check_startup_cancelled(stop_requested)
        evidence = synchronize(net, name)
        wait_observation(net, name, evidence["snapshot"]["sequence"])
    start_service("relayer")
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        check_startup_cancelled(stop_requested)
        pid = owned_process("relayer")
        if pid is None:
            raise ValueError("relayer exited during startup; inspect its log")
        health = net.journal.get("health", "relayer", {})
        if (
            health.get("pid") == pid
            and time.time() - health.get("last_tick", 0) < 30
            and not health.get("errors")
        ):
            break
        time.sleep(0.5)
    else:
        raise PendingEvidence("relayer has not completed its first reconciliation pass")
    print(
        json.dumps(
            {
                "ready": True,
                "coordinator": net.coordinator,
                "provider": net.config["provider_url"],
                "manifest": str(DATA / "manifest.json"),
            },
            indent=2,
        )
    )


def require_ready(report):
    """Compose readiness requires initialized nodes and successful service work."""
    if not report.get("initialized"):
        raise LocalnetError("local environment has not finished initialization")
    for name in CHAINS:
        if report["nodes"][name]["state"] != "running":
            raise LocalnetError(f"{name} is not ready")
    for role in ("provider", "relayer"):
        if report["services"][role]["state"] != "running":
            raise LocalnetError(f"{role} is not ready")
    if report["coordinator_read_error"] is not None:
        raise LocalnetError("coordinator state is unavailable")


def serve_environment():
    """Own the complete local environment for the container's lifetime."""
    if os.environ.get("APODECT_CONTAINER") != "1":
        raise LocalnetError("serve requires the explicit APODECT_CONTAINER=1 runtime")
    # A shared volume may have only one container owner, even across PID namespaces.
    with (DATA.parent / ".container.lock").open("a") as owner:
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise LocalnetError(
                "another container owns this local environment"
            ) from error
        stopped = threading.Event()
        previous_handlers = {}
        for sig in (signal.SIGTERM, signal.SIGINT):
            previous_handlers[sig] = signal.signal(
                sig, lambda _signal, _frame: stopped.set()
            )
        started = False
        try:
            with (DATA.parent / ".lock").open("a") as lifecycle:
                try:
                    fcntl.flock(lifecycle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError as error:
                    raise LocalnetError(
                        "another local lifecycle command is active"
                    ) from error
                # Only PID records belong to the old container. Keep all financial
                # journals, signed transactions, identities and ledgers unchanged.
                for name in CHAINS:
                    Environment(name).process_file.unlink(missing_ok=True)
                for role in ("provider", "relayer"):
                    (DATA / f"{role}.process.json").unlink(missing_ok=True)
                started = True
                up(stopped)
            while not stopped.wait(2):
                with (DATA.parent / ".lock").open("a") as lifecycle:
                    try:
                        fcntl.flock(lifecycle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        # Smoke deliberately pauses/restarts services under this lock.
                        continue
                    for name in CHAINS:
                        if Environment(name).owned_process() is None:
                            raise LocalnetError(f"{name} exited; inspect its node log")
                    for role in ("provider", "relayer"):
                        if owned_process(role) is None:
                            raise LocalnetError(
                                f"{role} exited; inspect its service log"
                            )
        finally:
            try:
                if started:
                    with (DATA.parent / ".lock").open("a") as lifecycle:
                        deadline = time.monotonic() + 45
                        while True:
                            try:
                                fcntl.flock(lifecycle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                                break
                            except BlockingIOError as error:
                                if time.monotonic() >= deadline:
                                    raise LocalnetError(
                                        "active lifecycle command prevented graceful shutdown; "
                                        "data retained for reconciliation"
                                    ) from error
                                time.sleep(0.2)
                        # Finish submissions before terminating their native RPCs.
                        # A timeout keeps every journal available for reconciliation.
                        for role in ("relayer", "provider"):
                            stop_service(role)
                        for name in CHAINS:
                            Environment(name).down()
            finally:
                for sig, previous in previous_handlers.items():
                    signal.signal(sig, previous)


def status():
    nodes = {}
    for name in CHAINS:
        environment = Environment(name)
        pid = environment.owned_process()
        nodes[name] = {
            "pid": pid,
            "state": (
                "stopped"
                if pid is None
                else "running"
                if environment.healthy()
                else "unhealthy"
            ),
            "rpc": environment.url(),
        }
    if not (DATA / "manifest.json").exists():
        return {"initialized": False, "nodes": nodes}
    net = network()
    provider_journal = network("provider").journal
    services = {}
    for role in ("provider", "relayer"):
        journal = provider_journal if role == "provider" else net.journal
        health = journal.get("health", role, {})
        pid = owned_process(role)
        fresh = (
            pid is not None
            and health.get("pid") == pid
            and time.time() - health.get("last_tick", 0) < 120
        )
        services[role] = {
            **health,
            "pid": pid,
            "state": "stopped"
            if pid is None
            else "degraded"
            if health.get("errors") or not fresh
            else "running",
            "heartbeat_fresh": fresh,
        }
    coordinator, domains, read_error = None, {}, None
    if nodes["near"]["state"] == "running":
        try:
            coordinator = net.view("get_state")
            domains = {
                name: net.view("get_domain", domain=raw_id(binding["domain"]))
                for name, binding in net.config["chains"].items()
            }
        except (OSError, PendingEvidence, LocalnetError) as error:
            read_error = str(error)
    return {
        "initialized": True,
        "nodes": nodes,
        "coordinator": coordinator,
        "coordinator_read_error": read_error,
        "services": services,
        "domains": domains,
        "plans": [value for _, value in net.journal.records("plan_state")],
        "unverified_provider_requests": [
            {"request_id": key, **value}
            for key, value in provider_journal.records("provider_request")
            if value.get("state") not in ("Answered", "Closed")
        ][-10:],
        "unresolved_payments": net.journal.unresolved() + provider_journal.unresolved(),
        "errors": {
            kind: [
                (key, error)
                for key, error in (
                    provider_journal if kind == "transport_error" else net.journal
                ).records(kind)
                if "resolved_at" not in error
            ][-5:]
            for kind in (
                "relay_error",
                "plan_error",
                "plan_recovery_error",
                "collection_error",
                "transport_error",
                "scan_error",
                "funding_error",
            )
        },
    }


def restart_services():
    for role in ("relayer", "provider"):
        stop_service(role)
    for role in ("provider", "relayer"):
        start_service(role)


@contextmanager
def suspend_relayer():
    """Configure an acceptance fault before any command can move its tokens."""
    stop_service("relayer")
    try:
        yield
    finally:
        start_service("relayer")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    for action in (
        "up",
        "serve",
        "down",
        "logs",
        "serve-provider",
        "serve-relayer",
        "check",
        "smoke",
    ):
        sub.add_parser(action)
    status_parser = sub.add_parser("status")
    status_parser.add_argument("--require-ready", action="store_true")
    reset = sub.add_parser("reset")
    reset.add_argument("--yes", action="store_true")
    request = sub.add_parser("request")
    request.add_argument("chain", choices=CHAINS)
    request.add_argument("amount", type=int, help="USDC base units (6 decimals)")
    for action in ("approve", "apply"):
        command = sub.add_parser(action)
        command.add_argument("chain", choices=CHAINS)
        command.add_argument("request")
        if action == "apply":
            command.add_argument("--source", choices=CHAINS)
    agent = sub.add_parser("agent")
    agent.add_argument("chain", choices=CHAINS)
    agent.add_argument("task")
    agent.add_argument(
        "method", choices=("request", "withdraw", "return", "release", "finalize")
    )
    agent.add_argument("amount", type=int, nargs="?")
    funding = sub.add_parser("fund")
    funding.add_argument("chain", choices=CHAINS)
    funding.add_argument("amount", type=int)
    funding.add_argument("--invoice", default="0")
    funding.add_argument(
        "--id",
        dest="intent_id",
        help="stable funding intent ID; reuse to reconcile without another transfer",
    )
    call = sub.add_parser("call")
    call.add_argument("method")
    call.add_argument(
        "--role", default="authority", choices=("owner", "authority", "agent")
    )
    call.add_argument("--args", default="{}")
    fault = sub.add_parser("fault")
    fault.add_argument("quote_id")
    fault.add_argument("mode")
    fault.add_argument("--release", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    if DATA.is_symlink() or DATA.parent.is_symlink():
        parser.error("local environment directories must not be symlinks")
    DATA.mkdir(parents=True, exist_ok=True, mode=0o700)
    if args.action.startswith("serve-"):
        serve(args.action.removeprefix("serve-"))
        return
    if args.action == "serve":
        serve_environment()
        return
    if args.action in ("up", "down", "reset", "smoke"):
        if args.action == "reset" and not args.yes:
            parser.error(
                "reset removes all local chains, deployments and keys; pass --yes"
            )
        # Keep the lifecycle lock outside directories removed by reset.
        with (DATA.parent / ".lock").open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise LocalnetError(
                    "another local lifecycle command is active"
                ) from error
            if args.action == "up":
                up()
            elif args.action == "smoke":
                from scenarios import smoke

                print(
                    json.dumps(
                        smoke(network(), restart_services, suspend_relayer), indent=2
                    )
                )
            else:
                for role in ("relayer", "provider"):
                    stop_service(role)
                for name in CHAINS:
                    environment = Environment(name)
                    environment.reset() if args.action == "reset" else environment.down()
                if args.action == "reset":
                    shutil.rmtree(DATA)
        return
    if args.action == "logs":
        for name in CHAINS:
            Environment(name).logs()
        for role in ("provider", "relayer"):
            path = DATA / f"{role}.log"
            if path.exists():
                with path.open(errors="replace") as output:
                    print(f"{role}:\n" + "".join(collections.deque(output, maxlen=40)))
        return
    if args.action == "status":
        report = status()
        print(json.dumps(report, indent=2))
        if args.require_ready:
            require_ready(report)
        return
    net = network()
    if args.action == "request":
        result = request_task(net, args.chain, args.amount)
    elif args.action == "approve":
        result = net.call(
            "approve",
            domain=raw_id(net.config["chains"][args.chain]["domain"]),
            request=raw_id(identity(args.request)),
        )
    elif args.action == "apply":
        result = apply_request(net, args.chain, args.request, source=args.source)
    elif args.action == "agent":
        if args.method != "finalize" and (args.amount is None or args.amount <= 0):
            parser.error(
                "this agent action requires a positive amount in USDC base units"
            )
        result = agent_action(net, args.chain, args.task, args.method, args.amount)
    elif args.action == "fund":
        result = fund(net, args.chain, args.amount, args.invoice, args.intent_id)
    elif args.action == "call":
        result = net.call(args.method, role=args.role, **json.loads(args.args))
    elif args.action == "fault":
        result = LiquidityClient(net.config["provider_url"]).fault(
            args.quote_id,
            args.mode,
            args.release,
        )
    else:
        from scenarios import check

        for chain in net.chains.values():
            chain.environment.check()
        result = check(net)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (
        ValueError,
        OSError,
        RuntimeError,
        KeyError,
        LocalnetError,
        subprocess.SubprocessError,
    ) as error:
        print(f"Local environment: {error}", file=sys.stderr)
        sys.exit(1)
