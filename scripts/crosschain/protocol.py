"""Apodect wire encoding and the pinned MPC 3.18.0 foreign-payload boundary.

No authorization decisions live here. Signatures authenticate bytes selected by
the coordinator; provider workers must independently verify their provenance.
"""

import hashlib
import json
import os
import struct
from enum import IntEnum
from pathlib import Path

from coincurve import PrivateKey, PublicKey

ZERO = "0x" + "00" * 32
U64_MAX = (1 << 64) - 1
SECP_ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
COMMAND_FIELDS = (
    "domain nonce action operation related task request amount limit recipient expires reference"
).split()
COMMAND_INTEGERS = {"nonce", "action", "amount", "limit", "expires"}
CHECKPOINT_FIELDS = (
    "domain observation sequence height timestamp free held "
    "task task_state paused started available issued recovered pending_outflow control_nonce agent "
    "request request_task request_amount request_kind request_state purpose "
    "plan plan_state plan_held route route_plan route_kind route_expected route_received "
    "route_dispatched operation command_digest command_action command_state command_result "
    "live_request live_request_exists"
).split()
CHECKPOINT_IDENTITIES = {
    "domain",
    "observation",
    "task",
    "agent",
    "request",
    "request_task",
    "purpose",
    "plan",
    "route",
    "route_plan",
    "operation",
    "command_digest",
    "command_result",
    "live_request",
}
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


class Action(IntEnum):
    RESERVE = 1
    EXPECT_DELIVERY = 2
    EXPECT_REFUND = 3
    DISPATCH = 4
    ALLOCATE = 5
    CLOSE_PLAN = 6
    REJECT = 7
    OUT_OF_BUDGET = 8
    PAUSE = 9
    RECLAIM = 10
    CONTINUE = 11
    CANCEL = 12
    RESTORE_REQUEST = 13
    OWNER_WITHDRAW = 14
    INVALIDATE = 15


class CommandState(IntEnum):
    MISSING = 0
    SUCCEEDED = 1
    FAILED = 2
    INVALIDATED = 3
    PENDING = 4

    @property
    def terminal(self):
        return self in (self.SUCCEEDED, self.FAILED, self.INVALIDATED)


def unhex(value, size=None):
    if not isinstance(value, str):
        raise ValueError("hex value must be a string")
    result = bytes.fromhex(value.removeprefix("0x"))
    if size is not None and len(result) != size:
        raise ValueError(f"expected {size} bytes, got {len(result)}")
    return result


def hex_bytes(value):
    return "0x" + bytes(value).hex()


def integer_word(value):
    if isinstance(value, bool):
        value = int(value)
    number = int(value)
    if not 0 <= number <= U64_MAX:
        raise ValueError("integer exceeds the shared u64 range")
    return number.to_bytes(32, "big")


def identity(value):
    if isinstance(value, int) or (
        isinstance(value, str) and value.isdecimal() and len(value) != 64
    ):
        number = int(value)
        if not 0 <= number < (1 << 256):
            raise ValueError("identity integer exceeds one unsigned 32-byte word")
        return hex_bytes(number.to_bytes(32, "big"))
    return hex_bytes(unhex(value, 32))


def encode_command(command):
    unknown = set(command) - set(COMMAND_FIELDS)
    if unknown:
        raise ValueError(f"unexpected command fields: {sorted(unknown)}")
    return b"".join(
        integer_word(command.get(field, 0))
        if field in COMMAND_INTEGERS
        else unhex(command.get(field, ZERO), 32)
        for field in COMMAND_FIELDS
    )


def decode_command(encoded):
    raw = unhex(encoded, 384) if isinstance(encoded, str) else bytes(encoded)
    if len(raw) != 384:
        raise ValueError("command must contain twelve words")
    result = {}
    for index, field in enumerate(COMMAND_FIELDS):
        word = raw[index * 32 : (index + 1) * 32]
        value = (
            int.from_bytes(word, "big")
            if field in COMMAND_INTEGERS
            else hex_bytes(word)
        )
        if field in COMMAND_INTEGERS and value > U64_MAX:
            raise ValueError("noncanonical integer word")
        result[field] = value
    Action(result["action"])
    return result


def decode_checkpoint(encoded):
    raw = unhex(encoded, 1248) if isinstance(encoded, str) else bytes(encoded)
    if len(raw) != len(CHECKPOINT_FIELDS) * 32:
        raise ValueError("checkpoint must contain exactly 39 words")
    result = {}
    for index, field in enumerate(CHECKPOINT_FIELDS):
        word = raw[index * 32 : (index + 1) * 32]
        result[field] = (
            hex_bytes(word)
            if field in CHECKPOINT_IDENTITIES
            else int.from_bytes(word, "big")
        )
        if field not in CHECKPOINT_IDENTITIES and result[field] > U64_MAX:
            raise ValueError(f"noncanonical checkpoint {field}")
    for field in ("paused", "started", "live_request_exists"):
        if result[field] not in (0, 1):
            raise ValueError(f"noncanonical boolean {field}")
    return result


def decode_funding(encoded):
    raw = unhex(encoded, 160) if isinstance(encoded, str) else bytes(encoded)
    if len(raw) != 160:
        raise ValueError("funding receipt must contain five words")
    fields = ("domain", "funding_id", "invoice_id", "amount", "payer")
    result = {
        field: hex_bytes(raw[index * 32 : (index + 1) * 32])
        for index, field in enumerate(fields)
    }
    result["amount"] = int(result["amount"], 16)
    if not 0 < result["amount"] <= U64_MAX or result["funding_id"] == ZERO:
        raise ValueError("invalid funding receipt amount/identity")
    return result


def digest(value):
    return hashlib.sha256(value).digest()


def canonical_json(value):
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()


def domain(coordinator, near_genesis, project_id, chain, genesis, vault, asset):
    if chain not in ("ethereum", "solana", "near"):
        raise ValueError("unsupported chain")
    project = unhex(project_id, 32).hex()
    fields = [
        "apodect.coordination.v1",
        coordinator,
        near_genesis,
        project,
        chain,
        genesis,
        vault,
        asset,
    ]
    if any(
        not isinstance(field, str) or "\n" in field or not field for field in fields
    ):
        raise ValueError("invalid coordination domain component")
    return hex_bytes(digest(("\n".join(fields) + "\n").encode()))


def base58_decode(value, size=None):
    number = 0
    for char in value:
        number = number * 58 + BASE58.index(char)
    body = number.to_bytes((number.bit_length() + 7) // 8, "big")
    result = b"\0" * (len(value) - len(value.lstrip("1"))) + body
    if size is not None and len(result) != size:
        raise ValueError("wrong base58 identity size")
    return result


def base58_encode(value):
    raw = bytes(value)
    number, encoded = int.from_bytes(raw, "big"), ""
    while number:
        number, remainder = divmod(number, 58)
        encoded = BASE58[remainder] + encoded
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + encoded


def private_key(path, create=False):
    """Persist only local mock keys, with restrictive permissions and no stdout."""
    path = Path(path)
    if create and not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(PrivateKey().secret)
            output.flush()
            os.fsync(output.fileno())
    if path.is_symlink() or path.stat().st_mode & 0o077:
        raise ValueError("mock signing key must be a private regular file")
    return PrivateKey(path.read_bytes())


def sign_mpc(key, message_hash):
    """Production Secp256k1 response shape; libsecp256k1 makes low-s signatures."""
    raw = unhex(message_hash, 32) if isinstance(message_hash, str) else message_hash
    signature = key.sign_recoverable(raw, hasher=None)
    if signature[64] > 1 or int.from_bytes(signature[32:64], "big") > SECP_ORDER // 2:
        raise ValueError("signature cannot be represented by the configured MPC domain")
    return {
        "scheme": "Secp256k1",
        "big_r": {
            "affine_point": bytes([2 + signature[64]]).hex() + signature[:32].hex()
        },
        "s": {"scalar": signature[32:64].hex()},
        "recovery_id": signature[64],
    }


def normalize_signature(response, message_hash, public_key):
    if response.get("scheme") != "Secp256k1":
        raise ValueError("unexpected MPC signature scheme")
    point = unhex(response["big_r"]["affine_point"], 33)
    scalar = unhex(response["s"]["scalar"], 32)
    recovery = response["recovery_id"]
    if recovery not in (0, 1) or point[0] != 2 + recovery:
        raise ValueError("invalid recovery point")
    if not 0 < int.from_bytes(scalar, "big") <= SECP_ORDER // 2:
        raise ValueError("noncanonical signature scalar")
    signature = point[1:] + scalar
    recovered = PublicKey.from_signature_and_message(
        signature + bytes([recovery]), unhex(message_hash, 32), hasher=None
    ).format(compressed=False)[1:]
    if recovered != unhex(public_key, 64):
        raise ValueError("wrong MPC public key")
    return hex_bytes(signature), recovery


def _u32(value):
    return struct.pack("<I", value)


def _u64(value):
    return struct.pack("<Q", value)


def _blob(value):
    return _u32(len(value)) + value


def foreign_payload(request, values):
    """Exact Borsh ForeignTxSignPayload::V1 for our two admitted extractors.

    Primary source: near/mpc tag 3.18.0, types/foreign_chain.rs. Unsupported
    variants/finality are rejected rather than guessed or serialized as JSON.
    """
    if len(request) != 1:
        raise ValueError("expected one foreign chain request")
    chain, args = next(iter(request.items()))
    if args["finality"] != "Finalized":
        raise ValueError("foreign evidence must be finalized")
    extractors = args["extractors"]
    if len(extractors) != len(values) or not extractors:
        raise ValueError("extractor/value count mismatch")
    if chain == "Ethereum":
        encoded = b"\x00\x01" + unhex(args["tx_id"], 32) + _u32(len(extractors))
        for extractor in extractors:
            if set(extractor) != {"Log"}:
                raise ValueError("only EVM Log evidence is admitted")
            encoded += b"\x01" + _u64(extractor["Log"]["log_index"])
        encoded += b"\x02" + _u32(len(values))
        for value in values:
            log = value["EvmExtractedValue"]["Log"]
            encoded += b"\x01\x01" + bytes([bool(log["removed"])])
            encoded += _u64(log["log_index"]) + _u64(log["transaction_index"])
            encoded += unhex(log["transaction_hash"], 32) + unhex(log["block_hash"], 32)
            encoded += _u64(log["block_number"]) + unhex(log["address"], 20)
            encoded += _blob(log["data"].encode()) + _u32(len(log["topics"]))
            encoded += b"".join(unhex(topic, 32) for topic in log["topics"])
    elif chain == "Solana":
        encoded = (
            b"\x00\x02" + unhex(args["tx_id"], 64) + b"\x01" + _u32(len(extractors))
        )
        for extractor in extractors:
            if set(extractor) != {"AccountState"}:
                raise ValueError(
                    "only immutable Solana AccountState evidence is admitted"
                )
            encoded += b"\x02" + unhex(extractor["AccountState"]["pubkey"], 32)
        encoded += _u32(len(values))
        for value in values:
            account = value["SvmExtractedValue"]["AccountState"]
            encoded += (
                b"\x06\x01"
                + unhex(account["owner"], 32)
                + _blob(unhex(account["data"]))
            )
    else:
        raise ValueError("unsupported foreign chain")
    return encoded


def foreign_hash(request, values):
    return hex_bytes(digest(foreign_payload(request, values)))


def raw_id(value):
    """JSON hex identities without the optional wire-display prefix."""
    return value.removeprefix("0x")
