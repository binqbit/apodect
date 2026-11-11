"""Export deployment addresses and dedicated gas credentials for Rust services."""

import json
import os
import subprocess
from pathlib import Path

from localnet import ROOT, Environment, LocalnetError, read_json, write_json


def export_runtime(network):
    ethereum_manifest = read_json(network.chains["ethereum"].environment.manifest_file)
    start_block = ethereum_manifest.get("project_start_block")
    if type(start_block) is not int or start_block < 0:
        raise LocalnetError(
            "Ethereum deployment lacks its creation block; reset and bootstrap "
            "the local environment before exporting the backend runtime"
        )
    directory = Path(
        os.environ.get("APODECT_BACKEND_RUNTIME", ROOT / ".localnet/backend-runtime")
    )
    if directory.is_symlink():
        raise ValueError("backend runtime directory must not be a symlink")
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    host = "localnet" if os.environ.get("APODECT_CONTAINER") == "1" else "127.0.0.1"
    accounts = {
        name: chain.bridge("relay_account") for name, chain in network.chains.items()
    }
    for name, account in accounts.items():
        binding = network.config["chains"][name]
        if account["address"] == binding["owner"]:
            raise ValueError("backend relay must not hold the Project owner key")

    ethereum = read_json(Path(accounts["ethereum"]["key_file"]))
    write_private(directory / "ethereum.key", ethereum["private_key"].encode() + b"\n")
    for name in ("solana", "near"):
        write_private(
            directory / f"{name}.json", Path(accounts[name]["key_file"]).read_bytes()
        )

    genesis = network.near.rpc("genesis_config", {})
    configuration = {
        "ethereum": {
            "rpc_url": f"http://{host}:{network.chains['ethereum'].environment.ports()['rpc']}",
            "chain_id": int(network.chains["ethereum"].rpc("eth_chainId", []), 16),
            "genesis_hash": network.chains["ethereum"].rpc(
                "eth_getBlockByNumber", ["0x0", False]
            )["hash"],
        },
        "solana": {
            "rpc_url": f"http://{host}:{network.chains['solana'].environment.ports()['rpc']}",
            "genesis_hash": network.chains["solana"].rpc("getGenesisHash", []),
        },
        "near": {
            "rpc_url": f"http://{host}:{network.chains['near'].environment.ports()['rpc']}",
            "genesis": {
                key: genesis[key]
                for key in ("chain_id", "genesis_time", "genesis_height", "validators")
            },
        },
        "projects": [
            {
                "coordinator": network.coordinator,
                "project_id": network.config["project_id"],
                "ethereum_start_block": start_block,
                "domains": {
                    name: {
                        key: binding[key]
                        for key in ("domain", "vault", "asset", "asset_id")
                    }
                    for name, binding in network.config["chains"].items()
                },
            }
        ],
        "liquidity": {
            "url": f"http://{host}:{network.config['provider_port']}",
            "local_simulator": True,
            "api_key_file": None,
        },
    }
    write_json(directory / "deployment.json", configuration)
    return {
        "configuration": str(directory / "deployment.json"),
        "relayer": accounts["near"]["address"],
    }


def write_private(path, contents):
    temporary = path.with_suffix(path.suffix + ".tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(contents)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)


def export_wallet(chain, role):
    """Explicitly export one disposable actor wallet for browser-only local signing."""
    if chain not in ("ethereum", "solana", "near") or role not in (
        "owner",
        "authority",
        "agent",
    ):
        raise LocalnetError("Choose a local chain and owner, authority or agent wallet")
    environment = Environment(chain)
    if chain == "ethereum":
        result = subprocess.run(
            [str(environment.adapter), "export-wallet", role],
            env=environment.environment(),
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode:
            raise LocalnetError(
                "Cannot export the local Ethereum wallet; check Foundry and local keys"
            )
        return json.loads(result.stdout)
    manifest = read_json(environment.manifest_file)
    account = manifest["accounts"][role]
    wallet = {"chain": chain, "address": account["address"]}
    if chain == "solana":
        wallet["secret_key"] = read_json(Path(account["key_file"]))
    else:
        wallet["private_key"] = read_json(Path(account["key_file"]))["private_key"]

    directory = ROOT / ".localnet" / "wallets"
    if directory.is_symlink():
        raise LocalnetError("Wallet export directory must not be a symlink")
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = directory / f"{role}.{chain}.json"
    write_private(path, json.dumps(wallet, indent=2).encode() + b"\n")
    return {"wallet_file": str(path), "chain": chain, "role": role}
