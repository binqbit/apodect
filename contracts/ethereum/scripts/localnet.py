#!/usr/bin/env python3
"""Native Anvil adapter; process ownership and reset belong to the root manager."""

import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import urllib.parse
import urllib.request


PACKAGE = Path(__file__).resolve().parents[1]
CHAIN_ID = 31337
CHAIN_MARKER = "0x0000000000000000000000000000000000a90dec"
UNIT = 10**6
ARTIFACTS = {
    "token": "out/LocalUSDC.s.sol/LocalUSDC.json",
    "registry": "out/WorkspaceRegistry.sol/WorkspaceRegistry.json",
    "project": "out/ProjectVault.sol/ProjectVault.json",
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def read_json(path):
    return json.loads(path.read_text())


def write_json(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.chmod(0o600)
    temporary.replace(path)


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
        require("error" not in payload, f"{method}: {payload.get('error')}")
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
            for role in ("owner", "authority", "agent"):
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
        run("forge", "soldeer", "install", "--config-location", "soldeer")
        run("forge", "build", "script/LocalUSDC.s.sol", "src/WorkspaceRegistry.sol")
        self.accounts()
        print(json.dumps({"chain": "ethereum", "prepared": True, "rpc_url": self.url}))

    def accounts(self):
        accounts = read_json(self.accounts_path)
        require(
            set(accounts) == {"owner", "authority", "agent"},
            "Invalid local account roles",
        )
        require(
            len({a["address"].lower() for a in accounts.values()}) == 3,
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
        os.execvp(
            "anvil",
            [
                "anvil",
                "--host",
                "127.0.0.1",
                "--port",
                str(self.port),
                "--chain-id",
                str(CHAIN_ID),
                "--hardfork",
                "cancun",
                "--accounts",
                "0",
                "--quiet",
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

    def transact(self, role, target=None, signature=None, *args, creation=None):
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
        return receipt

    def values(self, target, signature, *args):
        data = run("cast", "calldata", signature, *map(str, args))
        result = self.rpc("eth_call", [{"to": target, "data": data}, "latest"])[2:]
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
        self.transact(
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
        self.transact("owner", token, "approve(address,uint256)", project, 2**256 - 1)
        self.transact("owner", project, "fund(uint256,uint256)", 10_000 * UNIT, 0)
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
        balance = self.values(token, "balanceOf(address)", project)[0]
        require(
            free + allocated <= balance, "Project ledger exceeds actual token custody"
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

    def smoke(self):
        manifest = self.check()
        token, project = (manifest["contracts"][name] for name in ("token", "project"))
        accounts = manifest["accounts"]
        require(
            self.values(project, "queueHead()")[0]
            == self.values(project, "queueLength()")[0],
            "Smoke requires an empty Project queue; finish existing work first",
        )
        allocation, withdrawn, returned, released = (
            60 * UNIT,
            40 * UNIT,
            15 * UNIT,
            20 * UNIT,
        )
        free_before = self.values(project, "freeBalance()")[0]
        if free_before < allocation:
            top_up = 10_000 * UNIT
            self.transact(
                "owner",
                token,
                "mint(address,uint256)",
                accounts["owner"]["address"],
                top_up,
            )
            self.transact("owner", project, "fund(uint256,uint256)", top_up, 0)
            free_before += top_up
        purpose = "0x" + secrets.token_hex(32)
        self.transact(
            "agent", project, "createTask(uint256,bytes32)", allocation, purpose
        )
        task_id = self.values(project, "taskCount()")[0]
        request_id = self.values(project, "getTask(uint256)", task_id)[7]
        self.transact("authority", project, "approveRequest(uint256)", request_id)
        require(
            self.values(project, "freeBalance()")[0] == free_before,
            "Approval unexpectedly reserved principal",
        )
        self.transact("authority", project, "applyNext()")
        require(
            self.values(project, "getTask(uint256)", task_id)[4] == allocation,
            "Full allocation was not assigned",
        )
        agent_before = self.values(
            token, "balanceOf(address)", accounts["agent"]["address"]
        )[0]
        self.transact("agent", project, "withdraw(uint256,uint256)", task_id, withdrawn)
        self.transact("agent", token, "approve(address,uint256)", project, returned)
        self.transact(
            "agent", project, "returnFunds(uint256,uint256)", task_id, returned
        )
        self.transact("agent", project, "release(uint256,uint256)", task_id, released)
        self.transact("agent", project, "finalize(uint256,bytes32)", task_id, purpose)
        task = self.values(project, "getTask(uint256)", task_id)
        expense = withdrawn - returned
        require(
            task[1] == 4 and task[3] == 1 and task[4] == 0 and task[7] == 0,
            "Task did not close its rights",
        )
        require(
            task[5] == allocation and task[6] == returned + released,
            "Task principal counters differ",
        )
        require(
            self.values(project, "taskExpense(uint256)", task_id)[0] == expense,
            "Expense mismatch",
        )
        require(
            self.values(project, "freeBalance()")[0] == free_before - expense,
            "Project accounting mismatch",
        )
        require(
            self.values(token, "balanceOf(address)", accounts["agent"]["address"])[0]
            == agent_before + expense,
            "Agent's actual token balance does not match the lifecycle",
        )
        result = {
            "chain": "ethereum",
            "smoke": "passed",
            "task_id": task_id,
            "request_id": request_id,
            "expense": str(expense),
            "block_number": int(self.rpc("eth_blockNumber", []), 16),
        }
        write_json(self.directory / "last-smoke.json", result)
        print(json.dumps(result))


def main():
    os.umask(0o077)
    require(
        len(sys.argv) == 2
        and sys.argv[1] in {"prepare", "serve", "bootstrap", "check", "smoke"},
        "Usage: localnet.sh {prepare|serve|bootstrap|check|smoke}",
    )
    getattr(LocalEthereum(), sys.argv[1])()


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print(f"Ethereum local environment: {error}", file=sys.stderr)
        sys.exit(1)
