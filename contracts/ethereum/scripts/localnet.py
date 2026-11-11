#!/usr/bin/env python3
"""Native Anvil adapter; process ownership and reset belong to the root manager."""

import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]
CHAIN_ID = 31337
CHAIN_MARKER = "0x0000000000000000000000000000000000a90dec"
UNIT = 10**6
ARTIFACTS = {
    "token": "out/LocalUSDC.s.sol/LocalUSDC.json",
    "registry": "out/WorkspaceRegistry.sol/WorkspaceRegistry.json",
    "project": "out/ProjectVault.sol/ProjectVault.json",
    "escrow": "out/RouteEscrow.sol/RouteEscrow.json",
}


class RpcError(RuntimeError):
    """Retain structured RPC details so read retries admit only the expected error."""

    def __init__(self, method, error):
        super().__init__(f"{method}: {error}")
        self.code = error.get("code")
        self.message = error.get("message", "")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def read_json(path):
    return json.loads(path.read_text())


def write_json(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w") as handle:
        os.fchmod(handle.fileno(), 0o600)
        handle.write(json.dumps(data, indent=2) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    temporary.replace(path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def run(*args, env=None):
    result = subprocess.run(
        [str(arg) for arg in args],
        cwd=PACKAGE,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    require(result.returncode == 0, f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


class LocalEthereum:
    def __init__(self):
        directory = os.environ.get("APODECT_LOCALNET_DIR", "")
        require(
            directory and Path(directory).is_absolute(),
            "APODECT_LOCALNET_DIR must be absolute",
        )
        self.directory = Path(directory).resolve()
        self.port = int(os.environ.get("APODECT_RPC_PORT", "8545"))
        require(1024 <= self.port <= 65535, "RPC port must be between 1024 and 65535")
        self.url = os.environ.get("APODECT_RPC_URL", f"http://127.0.0.1:{self.port}")
        parsed = urllib.parse.urlsplit(self.url)
        require(
            parsed.scheme == "http"
            and parsed.hostname == "127.0.0.1"
            and parsed.port == self.port
            and parsed.path in ("", "/")
            and not parsed.username
            and not parsed.password
            and not parsed.query
            and not parsed.fragment,
            "Ethereum local RPC must be http://127.0.0.1:<APODECT_RPC_PORT>",
        )
        self.manifest_path = self.directory / "manifest.json"
        self.accounts_path = self.directory / "accounts.json"
        self.initial_state_path = self.directory / "initial-state.json"

    def rpc(self, method, params):
        request = urllib.request.Request(
            self.url,
            json.dumps(
                {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
            ).encode(),
            {"Content-Type": "application/json"},
        )
        # Never send local requests through a configured corporate/public HTTP proxy.
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(
            request, timeout=10
        ) as response:
            payload = json.load(response)
        if "error" in payload:
            raise RpcError(method, payload["error"])
        return payload["result"]

    def assert_local_chain(self):
        require(
            self.initial_state_path.exists(),
            "Run prepare before accessing this local chain",
        )
        require(
            "anvil" in self.rpc("web3_clientVersion", []).lower(), "RPC is not Anvil"
        )
        require(
            int(self.rpc("eth_chainId", []), 16) == CHAIN_ID,
            "Unexpected local chain ID",
        )
        expected = read_json(self.initial_state_path)["accounts"][CHAIN_MARKER]["code"]
        require(
            self.rpc("eth_getCode", [CHAIN_MARKER, "latest"]).lower()
            == expected.lower(),
            "RPC belongs to another local environment",
        )

    def prepare(self):
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        keys = self.directory / "keys"
        keys.mkdir(exist_ok=True, mode=0o700)
        require(
            self.accounts_path.exists() == self.initial_state_path.exists(),
            "Incomplete local identity setup; reset this environment before preparing it again",
        )
        if not self.accounts_path.exists():
            password = secrets.token_urlsafe(32)
            password_path = keys / "password.txt"
            password_path.write_text(password + "\n")
            password_path.chmod(0o600)
            accounts = {}
            for role in ("owner", "authority", "agent", "liquidity"):
                key_file = keys / f"{role}.json"
                require(
                    not key_file.exists(),
                    "Incomplete key setup; reset this environment",
                )
                env = dict(os.environ, CAST_PASSWORD=password)
                run("cast", "wallet", "new", keys, key_file.name, "--json", env=env)
                key_file.chmod(0o600)
                address = run(
                    "cast",
                    "wallet",
                    "address",
                    "--keystore",
                    key_file,
                    "--password-file",
                    password_path,
                )
                accounts[role] = {
                    "address": address,
                    "key_file": str(key_file),
                    "password_file": str(password_path),
                }
            write_json(self.accounts_path, accounts)
            allocations = {
                account["address"]: {
                    "nonce": 0,
                    "balance": hex(1000 * 10**18),
                    "code": "0x",
                    "storage": {},
                }
                for account in accounts.values()
            }
            allocations[CHAIN_MARKER] = {
                "nonce": 0,
                "balance": "0x0",
                "code": "0x00" + secrets.token_hex(32),
                "storage": {},
            }
            # Anvil's supported serialized-state format permits an accounts-only initial state.
            write_json(
                self.initial_state_path,
                {
                    "block": None,
                    "best_block_number": None,
                    "accounts": allocations,
                },
            )
        if os.environ.get("APODECT_CONTAINER") == "1":
            for name in ARTIFACTS:
                self.artifact(name)
        else:
            run("forge", "soldeer", "install", "--config-location", "soldeer")
            run("forge", "build", "script/LocalUSDC.s.sol", "src/WorkspaceRegistry.sol")
        self.accounts()
        print(json.dumps({"chain": "ethereum", "prepared": True, "rpc_url": self.url}))

    def accounts(self):
        accounts = read_json(self.accounts_path)
        require(
            set(accounts) == {"owner", "authority", "agent", "liquidity"},
            "Invalid local account roles",
        )
        require(
            len({a["address"].lower() for a in accounts.values()}) == len(accounts),
            "Local roles must use distinct accounts",
        )
        for account in accounts.values():
            for field in ("key_file", "password_file"):
                path = Path(account[field])
                require(
                    path.is_relative_to(self.directory / "keys") and path.is_file(),
                    "Missing local key material",
                )
                require(
                    path.stat().st_mode & 0o777 == 0o600,
                    "Local key files must have mode 0600",
                )
            actual = run(
                "cast",
                "wallet",
                "address",
                "--keystore",
                account["key_file"],
                "--password-file",
                account["password_file"],
            )
            require(
                actual.lower() == account["address"].lower(),
                "Local key does not match its recorded address",
            )
        return accounts

    def serve(self):
        require(self.initial_state_path.exists(), "Run prepare before serve")
        state = self.directory / "state.json"
        initial_state = state if state.exists() else self.initial_state_path
        # Empty blocks advance finality even when no user sends another transaction.
        os.execvp(
            "anvil",
            [
                "anvil",
                "--host",
                "0.0.0.0"
                if os.environ.get("APODECT_CONTAINER") == "1"
                else "127.0.0.1",
                "--port",
                str(self.port),
                "--chain-id",
                str(CHAIN_ID),
                "--hardfork",
                "cancun",
                "--accounts",
                "0",
                "--quiet",
                "--block-time",
                "1",
                "--slots-in-an-epoch",
                "1",
                "--mixed-mining",
                "--state-interval",
                "1",
                "--load-state",
                str(initial_state),
                "--dump-state",
                str(state),
            ],
        )

    def artifact(self, name):
        path = PACKAGE / ARTIFACTS[name]
        require(path.exists(), f"Missing {name} build artifact; run prepare")
        return read_json(path)

    def transact(
        self, role, target=None, signature=None, *args, creation=None, intent_id=None
    ):
        """Submit a native call and return only after finality.

        An intent records immutable signed bytes before broadcast. Its journal also
        blocks fresh transactions by that signer until the saved hash is finalized.
        The bridge dispatch lock serializes coordinated submissions across processes.
        """
        if intent_id is not None:
            require(creation is None, "Journaled intents require a contract call")
            return self._journaled_transaction(
                intent_id, role, target, signature, *args
            )
        self._require_clear_transactions(role)
        account = read_json(self.accounts_path)[role]
        command = [
            "cast",
            "send",
            "--rpc-url",
            self.url,
            "--no-proxy",
            "--chain",
            str(CHAIN_ID),
            "--keystore",
            account["key_file"],
            "--password-file",
            account["password_file"],
            "--json",
            "--timeout",
            "30",
        ]
        if creation is not None:
            command += ["--create", creation]
        else:
            command += [target, signature, *map(str, args)]
        receipt = json.loads(run(*command))
        require(int(receipt["status"], 16) == 1, "Local transaction reverted")
        self.wait_finalized(receipt)
        return receipt

    def _journaled_transaction(self, intent_id, role, target, signature, *args):
        """Persist before broadcast; retries reuse the exact signed bytes and nonce."""
        path = self.transaction_path(intent_id)
        binding = {
            "role": role,
            "target": target.lower(),
            "signature": signature,
            "args": list(map(str, args)),
        }
        if path.exists():
            record = read_json(path)
            require(
                record["binding"] == binding,
                "transfer_id was already used for another payment",
            )
        else:
            self._require_clear_transactions(role)
            account = read_json(self.accounts_path)[role]
            raw = run(
                "cast",
                "mktx",
                target,
                signature,
                *map(str, args),
                "--rpc-url",
                self.url,
                "--no-proxy",
                "--chain",
                CHAIN_ID,
                "--keystore",
                account["key_file"],
                "--password-file",
                account["password_file"],
            )
            require(raw.startswith("0x"), "Unexpected signed transaction encoding")
            record = {
                "binding": binding,
                "raw_transaction": raw,
                "tx_hash": run("cast", "keccak", raw),
            }
            path.parent.mkdir(mode=0o700, exist_ok=True)
            write_json(path, record)
        tx_hash = record["tx_hash"]
        receipt = self.rpc("eth_getTransactionReceipt", [tx_hash])
        if receipt is None:
            try:
                submitted = self.rpc(
                    "eth_sendRawTransaction", [record["raw_transaction"]]
                )
                require(submitted.lower() == tx_hash.lower(), "Broadcast hash mismatch")
            except RuntimeError:
                receipt = self.rpc("eth_getTransactionReceipt", [tx_hash])
                if receipt is None:
                    raise RuntimeError(
                        f"Payment outcome unknown; retain transfer_id and reconcile {tx_hash}"
                    ) from None
        for _ in range(100):
            if receipt is not None:
                require(int(receipt["status"], 16) == 1, f"Payment reverted: {tx_hash}")
                self.wait_finalized(receipt)
                return receipt
            time.sleep(0.1)
            receipt = self.rpc("eth_getTransactionReceipt", [tx_hash])
        raise RuntimeError(
            f"Payment outcome unknown; retain transfer_id and reconcile {tx_hash}"
        )

    def _require_clear_transactions(self, role):
        for path in (self.directory / "bridge").glob("payment-*.json"):
            pending = read_json(path)
            if pending["binding"]["role"] == role:
                receipt = self.rpc("eth_getTransactionReceipt", [pending["tx_hash"]])
                require(
                    receipt is not None,
                    f"Reconcile journaled {role} payment before a new transaction: {pending['tx_hash']}",
                )
                self.wait_finalized(receipt)

    def transaction_path(self, intent_id):
        require(
            isinstance(intent_id, str) and intent_id,
            "transfer_id must be a nonempty string",
        )
        return (
            self.directory
            / "bridge"
            / ("payment-" + hashlib.sha256(intent_id.encode()).hexdigest() + ".json")
        )

    def wait_finalized(self, receipt):
        """Verify inclusion in the canonical finalized block without resubmitting."""
        deadline = time.monotonic() + 30
        height = int(receipt["blockNumber"], 16)
        while time.monotonic() < deadline:
            finalized = self.rpc("eth_getBlockByNumber", ["finalized", False])
            if finalized is not None and int(finalized["number"], 16) >= height:
                block = self.rpc(
                    "eth_getBlockByNumber", [receipt["blockNumber"], False]
                )
                require(
                    block is not None
                    and block["hash"].lower() == receipt["blockHash"].lower(),
                    "Transaction receipt is not in the canonical finalized chain",
                )
                return
            time.sleep(0.2)
        raise RuntimeError(
            "Transaction included but not finalized; reconcile its existing hash: "
            + receipt["transactionHash"]
        )

    def values(self, target, signature, *args):
        data = run("cast", "calldata", signature, *map(str, args))
        deadline = time.monotonic() + 30
        while True:
            try:
                encoded = self.rpc(
                    "eth_call", [{"to": target, "data": data}, "finalized"]
                )
                break
            except RpcError as error:
                # Anvil restores the saved head's state without older state snapshots.
                # Its finalized tag can lag that head for two interval-mined blocks.
                # Retry only this read until finalized state exists; never use latest.
                if (
                    error.code != -32602
                    or not error.message.startswith("BlockOutOfRangeError:")
                    or time.monotonic() >= deadline
                ):
                    raise
                time.sleep(0.2)
        result = encoded[2:]
        require(result and len(result) % 64 == 0, f"Unexpected result from {signature}")
        return [
            int(result[index : index + 64], 16) for index in range(0, len(result), 64)
        ]

    def bootstrap(self):
        self.assert_local_chain()
        if self.manifest_path.exists():
            self.check()
            return
        marker = self.directory / "bootstrap-in-progress.json"
        require(
            not marker.exists(),
            "Interrupted bootstrap detected; reset this local environment before redeploying",
        )
        accounts = self.accounts()
        require(
            int(
                self.rpc(
                    "eth_getTransactionCount", [accounts["owner"]["address"], "latest"]
                ),
                16,
            )
            == 0,
            "Owner has prior transactions but no manifest; reset this local environment",
        )
        write_json(marker, {"chain": "ethereum", "state": "deploying"})
        token = self.transact(
            "owner", creation=self.artifact("token")["bytecode"]["object"]
        )["contractAddress"]
        constructor = run("cast", "abi-encode", "constructor(address)", token)[2:]
        registry = self.transact(
            "owner",
            creation=self.artifact("registry")["bytecode"]["object"] + constructor,
        )["contractAddress"]
        self.transact("owner", registry, "createWorkspace()")
        workspace_id = self.values(registry, "workspaceCount()")[0]
        project_receipt = self.transact(
            "owner",
            registry,
            "createProject(uint256,address)",
            workspace_id,
            accounts["authority"]["address"],
        )
        project = f"0x{self.values(registry, 'projects(uint256,uint256)', workspace_id, 1)[0]:040x}"
        self.transact(
            "owner",
            token,
            "mint(address,uint256)",
            accounts["owner"]["address"],
            1_000_000 * UNIT,
        )
        self.transact(
            "owner",
            token,
            "mint(address,uint256)",
            accounts["liquidity"]["address"],
            1_000_000 * UNIT,
        )
        contracts = {"token": token, "registry": registry, "project": project}
        artifacts = {}
        for name, address in contracts.items():
            artifact = self.artifact(name)
            artifacts[name] = {
                "path": ARTIFACTS[name],
                "creation_sha256": hashlib.sha256(
                    bytes.fromhex(artifact["bytecode"]["object"][2:])
                ).hexdigest(),
                "deployed_sha256": hashlib.sha256(
                    bytes.fromhex(self.rpc("eth_getCode", [address, "latest"])[2:])
                ).hexdigest(),
            }
        write_json(
            self.manifest_path,
            {
                "schema_version": 1,
                "chain": "ethereum",
                "chain_id": CHAIN_ID,
                "rpc_url": self.url,
                "contracts": contracts,
                "accounts": accounts,
                "asset": {"address": token, "decimals": 6},
                "workspace_id": workspace_id,
                "project_nonce": 1,
                "project_start_block": int(project_receipt["blockNumber"], 16),
                "artifacts": artifacts,
            },
        )
        marker.unlink()
        self.check()

    def check(self):
        self.assert_local_chain()
        require(
            self.manifest_path.exists(), "Deployment manifest is missing; run bootstrap"
        )
        manifest = read_json(self.manifest_path)
        require(
            manifest["schema_version"] == 1 and manifest["chain"] == "ethereum",
            "Invalid Ethereum manifest",
        )
        require(
            manifest["rpc_url"] == self.url and manifest["chain_id"] == CHAIN_ID,
            "Manifest RPC/chain mismatch",
        )
        accounts = self.accounts()
        require(
            manifest["accounts"] == accounts,
            "Manifest accounts differ from local identities",
        )
        for name, address in manifest["contracts"].items():
            artifact = self.artifact(name)
            code = self.rpc("eth_getCode", [address, "latest"])[2:]
            require(code, f"Missing deployed {name} contract")
            recorded = manifest["artifacts"][name]
            require(
                hashlib.sha256(bytes.fromhex(code)).hexdigest()
                == recorded["deployed_sha256"],
                f"Deployed {name} code changed",
            )
            require(
                hashlib.sha256(
                    bytes.fromhex(artifact["bytecode"]["object"][2:])
                ).hexdigest()
                == recorded["creation_sha256"],
                f"{name} artifact changed; reset and rebuild this local environment",
            )
            expected = artifact["deployedBytecode"]["object"][2:]
            for references in (
                artifact["deployedBytecode"].get("immutableReferences", {}).values()
            ):
                for reference in references:
                    start, end = (
                        reference["start"] * 2,
                        (reference["start"] + reference["length"]) * 2,
                    )
                    code = code[:start] + "0" * (end - start) + code[end:]
                    expected = expected[:start] + "0" * (end - start) + expected[end:]
            require(
                code == expected, f"{name} runtime does not match its compiled artifact"
            )
        token, registry, project = (
            manifest["contracts"][name] for name in ("token", "registry", "project")
        )
        require(
            self.values(token, "decimals()")[0] == 6
            and manifest["asset"] == {"address": token, "decimals": 6},
            "Asset identity/unit mismatch",
        )
        require(
            self.values(
                registry,
                "projects(uint256,uint256)",
                manifest["workspace_id"],
                manifest["project_nonce"],
            )[0]
            == int(project, 16),
            "Registry Project binding mismatch",
        )
        require(
            self.values(project, "asset()")[0] == int(token, 16),
            "Project asset mismatch",
        )
        require(
            self.values(project, "owner()")[0] == int(accounts["owner"]["address"], 16),
            "Project owner mismatch",
        )
        require(
            self.values(project, "authority()")[0]
            == int(accounts["authority"]["address"], 16),
            "Project authority mismatch",
        )
        free = self.values(project, "freeBalance()")[0]
        allocated = self.values(project, "allocatedBalance()")[0]
        held = self.values(project, "heldBalance()")[0]
        balance = self.values(token, "balanceOf(address)", project)[0]
        require(
            free + allocated + held <= balance,
            "Project ledger exceeds actual token custody",
        )
        print(
            json.dumps(
                {
                    "chain": "ethereum",
                    "ready": True,
                    "rpc_url": self.url,
                    "project": project,
                    "free_principal": str(free),
                    "allocated_principal": str(allocated),
                }
            )
        )
        return manifest

    def bridge(self):
        from bridge import EthereumBridge

        self.assert_local_chain()
        result = EthereumBridge(self).run(json.load(sys.stdin))
        print(json.dumps(result))

    def export_wallet(self, role):
        require(role in {"owner", "authority", "agent"}, "Unknown actor role")
        account = read_json(self.manifest_path)["accounts"][role]
        key_file = Path(account["key_file"])
        environment = os.environ.copy()
        environment["CAST_UNSAFE_PASSWORD"] = (
            Path(account["password_file"]).read_text().strip()
        )
        result = subprocess.run(
            [
                "cast",
                "wallet",
                "decrypt-keystore",
                key_file.name,
                "--keystore-dir",
                str(key_file.parent),
            ],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        # Captured output contains a private key and must never be logged.
        match = re.search(r"\b0x[0-9a-fA-F]{64}\b", result.stdout)
        require(
            result.returncode == 0 and match is not None,
            "Cannot unlock local test wallet",
        )
        directory = self.directory.parent / "wallets"
        require(not directory.is_symlink(), "Wallet directory must not be a symlink")
        directory.mkdir(exist_ok=True, mode=0o700)
        path = directory / f"{role}.ethereum.json"
        write_json(
            path,
            {
                "chain": "ethereum",
                "address": account["address"],
                "private_key": match.group(0),
            },
        )
        print(json.dumps({"wallet_file": str(path), "chain": "ethereum", "role": role}))


def main():
    os.umask(0o077)
    if len(sys.argv) == 3 and sys.argv[1] == "export-wallet":
        LocalEthereum().export_wallet(sys.argv[2])
        return
    require(
        len(sys.argv) == 2
        and sys.argv[1] in {"prepare", "serve", "bootstrap", "check", "bridge"},
        "Usage: localnet.sh {prepare|serve|bootstrap|check|bridge}",
    )
    getattr(LocalEthereum(), sys.argv[1])()


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print(f"Ethereum local environment: {error}", file=sys.stderr)
        sys.exit(1)
