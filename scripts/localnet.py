#!/usr/bin/env python3
"""Manage persistent, loopback-only development chains and their local deployments."""

import argparse
import collections
import contextlib
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
CHAINS = ("ethereum", "solana", "near")
DEFAULT_PORTS = {
    "ethereum": {"rpc": 8545},
    "solana": {"rpc": 8899, "faucet": 9900},
    "near": {"rpc": 3030, "network": 24567},
}


class LocalnetError(Exception):
    """An actionable local environment error."""


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise LocalnetError(f"Cannot read {path}: {error}") from error


def write_json(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.chmod(0o600)
    temporary.replace(path)


def process_identity(pid):
    """Include process start time so an unrelated reused PID is never signalled."""
    if pid <= 1:
        return None
    try:
        if Path("/proc").is_dir():
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            if fields[0] == "Z":
                return None
            return fields[19]
        result = subprocess.run(
            ["ps", "-p", str(pid), "-o", "lstart="],
            capture_output=True,
            text=True,
            check=False,
        )
        return result.stdout.strip() or None
    except (OSError, IndexError):
        return None


def configured_ports(chain):
    ports = {}
    for name, default in DEFAULT_PORTS[chain].items():
        variable = f"APODECT_{chain.upper()}_{name.upper()}_PORT"
        raw = os.environ.get(variable, str(default))
        try:
            port = int(raw)
        except ValueError as error:
            raise LocalnetError(f"{variable} must be an integer") from error
        if not 1024 <= port <= 65534:
            raise LocalnetError(f"{variable} must be between 1024 and 65534")
        ports[name] = port
    if chain == "solana":
        ports["websocket"] = ports["rpc"] + 1
    if len(set(ports.values())) != len(ports):
        raise LocalnetError(f"{chain}: ports must be distinct")
    return ports


def require_free_ports(ports):
    # Keep bindings until every port has been checked, including duplicate ports.
    with contextlib.ExitStack() as stack:
        for name, port in ports.items():
            sock = stack.enter_context(socket.socket())
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                sock.bind(("127.0.0.1", port))
            except OSError as error:
                raise LocalnetError(
                    f"{name} port {port} is unavailable; no existing process was stopped"
                ) from error


class Environment:
    def __init__(self, chain, root=ROOT):
        self.chain = chain
        self.root = root
        self.directory = root / ".localnet" / chain
        self.adapter = root / "contracts" / chain / "scripts" / "localnet.sh"
        self.process_file = self.directory / "process.json"
        self.settings_file = self.directory / "settings.json"
        self.manifest_file = self.directory / "manifest.json"
        self.log_file = self.directory / "node.log"
        self.child = None

    def ports(self):
        if self.settings_file.exists():
            return read_json(self.settings_file)["ports"]
        return configured_ports(self.chain)

    def url(self):
        return f"http://127.0.0.1:{self.ports()['rpc']}"

    def environment(self):
        environment = os.environ.copy()
        ports = self.ports()
        environment.update(
            {
                "APODECT_LOCALNET_DIR": str(self.directory),
                "APODECT_RPC_PORT": str(ports["rpc"]),
                "APODECT_RPC_URL": self.url(),
            }
        )
        for name in ("faucet", "network"):
            if name in ports:
                environment[f"APODECT_{name.upper()}_PORT"] = str(ports[name])
        return environment

    def adapter_command(self, command):
        result = subprocess.run(
            [str(self.adapter), command],
            cwd=self.root,
            env=self.environment(),
            check=False,
        )
        if result.returncode:
            raise LocalnetError(
                f"{self.chain}: {command} failed (exit {result.returncode})"
            )

    def owned_process(self):
        if not self.process_file.exists():
            return None
        record = read_json(self.process_file)
        pid = record["pid"]
        identity = process_identity(pid)
        if identity is None:
            return None
        try:
            group = os.getpgid(pid)
        except ProcessLookupError:
            return None
        if identity != record["identity"] or group != pid:
            raise LocalnetError(
                f"{self.chain}: process ownership changed; refusing to signal PID {pid}"
            )
        return pid

    def healthy(self):
        method = {"ethereum": "eth_chainId", "solana": "getHealth", "near": "status"}
        request = urllib.request.Request(
            self.url(),
            data=json.dumps(
                {"jsonrpc": "2.0", "id": 1, "method": method[self.chain], "params": []}
            ).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open(request, timeout=2) as response:
                result = json.load(response)
            if "error" in result or "result" not in result:
                return False
            if self.chain == "ethereum":
                return result["result"] == "0x7a69"  # Anvil development chain 31337.
            if self.chain == "solana":
                return result["result"] == "ok"
            return result["result"].get("sync_info", {}).get("syncing") is False
        except (OSError, ValueError, urllib.error.URLError):
            return False

    def wait_ready(self, child):
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise LocalnetError(
                    f"{self.chain}: node exited; inspect {self.log_file}"
                )
            if self.healthy():
                return
            time.sleep(1)
        raise LocalnetError(
            f"{self.chain}: RPC startup timed out; inspect {self.log_file}"
        )

    def validate_manifest(self):
        manifest = read_json(self.manifest_file)
        if (
            manifest.get("schema_version") != 1
            or manifest.get("chain") != self.chain
            or manifest.get("rpc_url") != self.url()
        ):
            raise LocalnetError(
                f"{self.chain}: deployment manifest does not match this environment"
            )
        if not manifest.get("contracts") or not manifest.get("asset", {}).get(
            "address"
        ):
            raise LocalnetError(
                f"{self.chain}: deployment manifest lacks contract/asset addresses"
            )
        for role in ("owner", "authority", "agent"):
            account = manifest.get("accounts", {}).get(role, {})
            if not account.get("address") or not account.get("key_file"):
                raise LocalnetError(f"{self.chain}: manifest lacks the {role} identity")
            key_file = Path(account["key_file"])
            if not key_file.is_file() or not key_file.resolve().is_relative_to(
                self.directory.resolve()
            ):
                raise LocalnetError(
                    f"{self.chain}: missing or external {role} key file"
                )
        return manifest

    def up(self):
        selected_ports = configured_ports(self.chain)
        if self.settings_file.exists() and self.ports() != selected_ports:
            raise LocalnetError(
                f"{self.chain}: port settings changed; use the existing ports or reset this chain"
            )
        if self.owned_process():
            self.check()
            print(f"{self.chain}: already running at {self.url()}", flush=True)
            return
        require_free_ports(selected_ports)
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.directory.chmod(0o700)
        write_json(self.settings_file, {"ports": selected_ports})
        print(f"{self.chain}: preparing contracts and node", flush=True)
        self.adapter_command("prepare")
        require_free_ports(selected_ports)
        with self.log_file.open("ab") as log:
            self.log_file.chmod(0o600)
            child = subprocess.Popen(
                [str(self.adapter), "serve"],
                cwd=self.root,
                env=self.environment(),
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        self.child = child
        try:
            identity = process_identity(child.pid)
            if identity is None:
                raise LocalnetError(
                    f"{self.chain}: node failed to start; inspect {self.log_file}"
                )
            write_json(self.process_file, {"pid": child.pid, "identity": identity})
            print(f"{self.chain}: waiting for RPC at {self.url()}", flush=True)
            self.wait_ready(child)
            if not self.manifest_file.exists():
                self.adapter_command("bootstrap")
            self.check()
        except BaseException:
            # Keep the actual child handle: writing process.json can itself fail.
            # poll() establishes this is still our unreaped child, independently
            # of persistent metadata, before signalling its process group.
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=30)
                except subprocess.TimeoutExpired as error:
                    raise LocalnetError(
                        f"{self.chain}: startup cleanup timed out for PID {child.pid}; "
                        f"inspect {self.log_file}; data retained"
                    ) from error
            self.child = None
            self.process_file.unlink(missing_ok=True)
            raise
        print(f"{self.chain}: ready; deployment {self.manifest_file}", flush=True)

    def check(self):
        if not self.owned_process() or not self.healthy():
            raise LocalnetError(
                f"{self.chain}: managed node is not healthy; run local-up first"
            )
        self.validate_manifest()
        self.adapter_command("check")

    def smoke(self):
        self.check()
        self.adapter_command("smoke")
        self.check()

    def down(self):
        pid = self.owned_process()
        if pid is None:
            if self.child is not None and self.child.poll() is not None:
                self.child = None
            self.process_file.unlink(missing_ok=True)
            print(f"{self.chain}: stopped (data retained)", flush=True)
            return
        # The process is its own session/group leader; stop only this owned group.
        try:
            os.killpg(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + 30
        while self.owned_process() is not None:
            if time.monotonic() >= deadline:
                raise LocalnetError(
                    f"{self.chain}: graceful shutdown timed out; data retained, inspect {self.log_file}"
                )
            time.sleep(0.2)
        self.process_file.unlink(missing_ok=True)
        if self.child is not None:
            self.child.wait(timeout=5)
            self.child = None
        print(f"{self.chain}: stopped (data retained)", flush=True)

    def status(self):
        pid = self.owned_process()
        state = "stopped"
        if pid:
            state = "running" if self.healthy() else "starting/unhealthy"
        deployment = "deployed" if self.manifest_file.exists() else "not deployed"
        print(
            f"{self.chain}: {state}, {deployment}, {self.url()}"
            + (f", PID {pid}" if pid else ""),
            flush=True,
        )
        return state != "starting/unhealthy"

    def reset(self):
        self.down()
        if self.directory.exists():
            require_free_ports(self.ports())
            if self.directory.is_symlink():
                raise LocalnetError(
                    f"Refusing to delete a symbolic link: {self.directory}"
                )
            shutil.rmtree(self.directory)
        print(
            f"{self.chain}: local chain, deployment and disposable keys removed",
            flush=True,
        )

    def logs(self):
        print(f"--- {self.chain}: {self.log_file} ---", flush=True)
        if not self.log_file.exists():
            print("No node log yet.")
            return
        with self.log_file.open(errors="replace") as log:
            print("".join(collections.deque(log, maxlen=60)), end="", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command", choices=("up", "down", "status", "check", "smoke", "reset", "logs")
    )
    parser.add_argument("chain", nargs="?", default="all", choices=("all", *CHAINS))
    parser.add_argument("--yes", action="store_true", help="confirm deletion for reset")
    args = parser.parse_args()
    if args.command == "reset" and not args.yes:
        parser.error(
            "reset deletes selected chain data and local keys; pass --yes to confirm"
        )
    os.umask(0o077)
    data_root = ROOT / ".localnet"
    if data_root.is_symlink():
        parser.error(
            ".localnet must be a directory inside this checkout, not a symbolic link"
        )
    data_root.mkdir(mode=0o700, exist_ok=True)
    chains = CHAINS if args.chain == "all" else (args.chain,)
    failed = False
    with (data_root / ".lock").open("a") as lock:
        if args.command not in ("status", "logs"):
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print(
                    "Another localnet command is active in this checkout.",
                    file=sys.stderr,
                )
                return 1
        for chain in chains:
            environment = Environment(chain)
            try:
                if environment.directory.is_symlink():
                    raise LocalnetError(
                        f"Refusing symbolic link: {environment.directory}"
                    )
                if getattr(environment, args.command)() is False:
                    failed = True
            except (LocalnetError, OSError, KeyError, TypeError) as error:
                print(f"Error: {error}", file=sys.stderr, flush=True)
                failed = True
            except KeyboardInterrupt:
                print("Interrupted. Existing chain data was retained.", file=sys.stderr)
                return 130
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
