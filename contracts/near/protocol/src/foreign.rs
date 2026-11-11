//! Narrow, explicit Read MPC 3.18.0 Borsh boundary: finalized EVM Log and SVM AccountState.

use crate::{Checkpoint, Word, hash, number, unhex, word};
use near_sdk::{env, require, serde_json::Value};

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect("MISSING_FOREIGN_STRING")
}
fn integer(value: &Value, key: &str) -> u64 {
    value[key].as_u64().expect("MISSING_FOREIGN_INTEGER")
}
fn bytes(value: &str, length: usize) -> Vec<u8> {
    let result = unhex(value.strip_prefix("0x").unwrap_or(value));
    require!(result.len() == length, "FOREIGN_BYTE_LENGTH");
    result
}
fn blob(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(
        &u32::try_from(value.len())
            .expect("FOREIGN_BLOB_TOO_LONG")
            .to_le_bytes(),
    );
    out.extend_from_slice(value);
}

/// Serialize precisely the admitted official enum variants, never arbitrary JSON.
pub fn foreign_hash(request: &Value, values: &Value) -> Word {
    let values = values.as_array().expect("FOREIGN_VALUES_ARRAY");
    require!(values.len() == 1, "ONE_EXTRACTOR_REQUIRED");
    let request = request.as_object().expect("FOREIGN_REQUEST_OBJECT");
    require!(request.len() == 1, "ONE_CHAIN_REQUIRED");
    let (chain, args) = request.iter().next().unwrap();
    require!(text(args, "finality") == "Finalized", "FINALIZED_REQUIRED");
    let extractors = args["extractors"].as_array().expect("EXTRACTORS_ARRAY");
    require!(extractors.len() == 1, "ONE_EXTRACTOR_REQUIRED");
    let mut out = vec![0]; // ForeignTxSignPayload::V1
    if chain == "Ethereum" {
        out.push(1);
        out.extend(bytes(text(args, "tx_id"), 32));
        out.extend(1u32.to_le_bytes());
        out.push(1);
        out.extend(integer(&extractors[0]["Log"], "log_index").to_le_bytes());
        out.push(2);
        out.extend(1u32.to_le_bytes());
        out.extend([1, 1]);
        let log = &values[0]["EvmExtractedValue"]["Log"];
        out.push(u8::from(log["removed"].as_bool().expect("REMOVED_BOOL")));
        out.extend(integer(log, "log_index").to_le_bytes());
        out.extend(integer(log, "transaction_index").to_le_bytes());
        out.extend(bytes(text(log, "transaction_hash"), 32));
        out.extend(bytes(text(log, "block_hash"), 32));
        out.extend(integer(log, "block_number").to_le_bytes());
        out.extend(bytes(text(log, "address"), 20));
        blob(&mut out, text(log, "data").as_bytes());
        let topics = log["topics"].as_array().expect("TOPICS_ARRAY");
        out.extend(u32::try_from(topics.len()).unwrap().to_le_bytes());
        for topic in topics {
            out.extend(bytes(topic.as_str().expect("TOPIC_STRING"), 32));
        }
    } else if chain == "Solana" {
        out.push(2);
        out.extend(bytes(text(args, "tx_id"), 64));
        out.push(1);
        out.extend(1u32.to_le_bytes());
        out.push(2);
        out.extend(bytes(text(&extractors[0]["AccountState"], "pubkey"), 32));
        out.extend(1u32.to_le_bytes());
        out.extend([6, 1]);
        let account = &values[0]["SvmExtractedValue"]["AccountState"];
        out.extend(bytes(text(account, "owner"), 32));
        blob(
            &mut out,
            &unhex(
                text(account, "data")
                    .strip_prefix("0x")
                    .unwrap_or(text(account, "data")),
            ),
        );
    } else {
        env::panic_str("UNSUPPORTED_FOREIGN_CHAIN");
    }
    hash(&out)
}

/// Bind the attested value to the registered application and exact checkpoint.
pub fn validate_evidence(
    chain: &str,
    vault: &str,
    program: Option<&str>,
    checkpoint: &Checkpoint,
    request: &Value,
    values: &Value,
) -> Word {
    let expected_hash = foreign_hash(request, values);
    let payload = unhex(&checkpoint.hex());
    if chain == "ethereum" {
        let args = &request["Ethereum"];
        require!(args.is_object(), "WRONG_FOREIGN_CHAIN");
        let log = &values[0]["EvmExtractedValue"]["Log"];
        require!(log["removed"] == false, "REMOVED_LOG");
        require!(
            bytes(text(log, "address"), 20) == bytes(vault, 20),
            "WRONG_LOG_EMITTER"
        );
        require!(
            bytes(text(log, "transaction_hash"), 32) == bytes(text(args, "tx_id"), 32),
            "WRONG_LOG_TRANSACTION"
        );
        require!(
            integer(log, "log_index") == integer(&args["extractors"][0]["Log"], "log_index"),
            "WRONG_LOG_INDEX"
        );
        require!(
            integer(log, "block_number") == checkpoint.value(3),
            "WRONG_LOG_HEIGHT"
        );
        let topics = log["topics"].as_array().expect("TOPICS_ARRAY");
        require!(topics.len() == 3, "WRONG_EVENT_TOPICS");
        let event = env::keccak256_array(b"CheckpointRecorded(bytes32,bytes32,bytes)");
        require!(
            bytes(topics[0].as_str().unwrap(), 32) == event,
            "WRONG_EVENT_SIGNATURE"
        );
        require!(
            bytes(topics[1].as_str().unwrap(), 32) == checkpoint.words[1],
            "WRONG_EVENT_OBSERVATION"
        );
        require!(
            bytes(topics[2].as_str().unwrap(), 32) == checkpoint.words[0],
            "WRONG_EVENT_DOMAIN"
        );
        let data = unhex(
            text(log, "data")
                .strip_prefix("0x")
                .unwrap_or(text(log, "data")),
        );
        require!(data.len() == 64 + payload.len(), "WRONG_EVENT_DATA_LENGTH");
        require!(
            number(data[..32].try_into().unwrap()) == 32
                && number(data[32..64].try_into().unwrap()) == payload.len() as u64,
            "NONCANONICAL_EVENT_ABI"
        );
        require!(data[64..] == payload, "EVENT_PAYLOAD_MISMATCH");
    } else {
        require!(
            chain == "solana" && request["Solana"].is_object(),
            "WRONG_FOREIGN_CHAIN"
        );
        validate_receipt_pda(
            vault,
            program.expect("SOLANA_PROGRAM_REQUIRED"),
            b"checkpoint",
            checkpoint.words[1],
            request,
        );
        let account = &values[0]["SvmExtractedValue"]["AccountState"];
        require!(
            bytes(text(account, "owner"), 32) == word(program.expect("SOLANA_PROGRAM_REQUIRED")),
            "WRONG_ACCOUNT_OWNER"
        );
        let data = unhex(
            text(account, "data")
                .strip_prefix("0x")
                .unwrap_or(text(account, "data")),
        );
        require!(data.len() == 8 + payload.len(), "WRONG_ACCOUNT_LENGTH");
        require!(
            data[..8] == hash(b"account:Checkpoint")[..8],
            "WRONG_ACCOUNT_DISCRIMINATOR"
        );
        require!(data[8..] == payload, "ACCOUNT_PAYLOAD_MISMATCH");
    }
    expected_hash
}

/// Funding has a separate immutable payload; it is never inferred from balance deltas.
pub fn validate_funding(
    chain: &str,
    vault: &str,
    program: Option<&str>,
    payload: &str,
    request: &Value,
    values: &Value,
) -> Word {
    let expected = foreign_hash(request, values);
    let raw = unhex(payload);
    require!(raw.len() == 160, "INVALID_FUNDING_LENGTH");
    if chain == "ethereum" {
        let args = &request["Ethereum"];
        require!(args.is_object(), "WRONG_FOREIGN_CHAIN");
        let log = &values[0]["EvmExtractedValue"]["Log"];
        require!(log["removed"] == false, "REMOVED_LOG");
        require!(
            bytes(text(log, "address"), 20) == bytes(vault, 20),
            "WRONG_LOG_EMITTER"
        );
        require!(
            bytes(text(log, "transaction_hash"), 32) == bytes(text(args, "tx_id"), 32),
            "WRONG_LOG_TRANSACTION"
        );
        require!(
            integer(log, "log_index") == integer(&args["extractors"][0]["Log"], "log_index"),
            "WRONG_LOG_INDEX"
        );
        let topics = log["topics"].as_array().expect("TOPICS_ARRAY");
        require!(topics.len() == 3, "WRONG_EVENT_TOPICS");
        require!(
            bytes(topics[0].as_str().unwrap(), 32)
                == env::keccak256_array(b"FundingRecorded(bytes32,bytes32,bytes)"),
            "WRONG_EVENT_SIGNATURE"
        );
        require!(
            bytes(topics[1].as_str().unwrap(), 32) == raw[32..64]
                && bytes(topics[2].as_str().unwrap(), 32) == raw[..32],
            "WRONG_FUNDING_TOPICS"
        );
        let data = unhex(text(log, "data"));
        require!(data.len() == 224, "WRONG_EVENT_DATA_LENGTH");
        require!(
            number(data[..32].try_into().unwrap()) == 32
                && number(data[32..64].try_into().unwrap()) == 160
                && data[64..] == raw,
            "WRONG_FUNDING_ABI"
        );
    } else {
        require!(
            chain == "solana" && request["Solana"].is_object(),
            "WRONG_FOREIGN_CHAIN"
        );
        validate_receipt_pda(
            vault,
            program.expect("SOLANA_PROGRAM_REQUIRED"),
            b"funding",
            raw[32..64].try_into().unwrap(),
            request,
        );
        let account = &values[0]["SvmExtractedValue"]["AccountState"];
        require!(
            bytes(text(account, "owner"), 32) == word(program.expect("SOLANA_PROGRAM_REQUIRED")),
            "WRONG_ACCOUNT_OWNER"
        );
        let data = unhex(text(account, "data"));
        require!(data.len() == 168, "WRONG_ACCOUNT_LENGTH");
        require!(
            data[..8] == hash(b"account:FundingReceipt")[..8] && data[8..] == raw,
            "WRONG_FUNDING_ACCOUNT"
        );
    }
    expected
}

fn solana_pubkey(value: &str) -> Word {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    require!(
        !value.is_empty() && value.len() <= 44,
        "INVALID_SOLANA_PUBLIC_KEY"
    );
    let mut key = [0u8; 32];
    for byte in value.bytes() {
        let mut carry = ALPHABET
            .iter()
            .position(|digit| *digit == byte)
            .expect("INVALID_SOLANA_BASE58") as u32;
        for digit in key.iter_mut().rev() {
            let next = u32::from(*digit) * 58 + carry;
            *digit = next as u8;
            carry = next >> 8;
        }
        require!(carry == 0, "SOLANA_PUBLIC_KEY_OVERFLOW");
    }
    require!(
        value.bytes().take_while(|byte| *byte == b'1').count()
            == key.iter().take_while(|byte| **byte == 0).count(),
        "NONCANONICAL_SOLANA_PUBLIC_KEY"
    );
    key
}

/// Owner checks alone are insufficient: another Project may pin the same opaque domain.
/// Bind immutable evidence to the PDA that the registered vault can actually create.
fn validate_receipt_pda(vault: &str, program: &str, seed: &[u8], id: Word, request: &Value) {
    let vault = solana_pubkey(vault);
    let program = word(program);
    let actual = word(text(
        &request["Solana"]["extractors"][0]["AccountState"],
        "pubkey",
    ));
    let mut input = [
        seed,
        vault.as_slice(),
        id.as_slice(),
        &[0],
        program.as_slice(),
        b"ProgramDerivedAddress",
    ]
    .concat();
    let bump_index = seed.len() + 64;
    let matched = (0..=255).rev().any(|bump| {
        input[bump_index] = bump;
        hash(&input) == actual
    });
    require!(matched, "RECEIPT_NOT_REGISTERED_VAULT_PDA");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(vault: Word, seed: &[u8], id: Word) -> Value {
        let program = [9u8; 32];
        let address = hash(
            &[
                seed,
                vault.as_slice(),
                id.as_slice(),
                &[255],
                program.as_slice(),
                b"ProgramDerivedAddress",
            ]
            .concat(),
        );
        near_sdk::serde_json::json!({
            "Solana": {
                "tx_id": "11".repeat(64),
                "finality": "Finalized",
                "extractors": [{
                    "AccountState": {
                        "pubkey": crate::hex(&address)
                    }
                }]
            }
        })
    }

    #[test]
    fn evidence_receipt_is_bound_to_registered_vault_seed() {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        let vault = "11111111111111111111111111111111";
        assert_eq!(solana_pubkey(vault), [0; 32]);
        for seed in [b"checkpoint".as_slice(), b"funding".as_slice()] {
            validate_receipt_pda(
                vault,
                &"09".repeat(32),
                seed,
                [3; 32],
                &request([0; 32], seed, [3; 32]),
            );
        }
    }

    #[test]
    #[should_panic(expected = "RECEIPT_NOT_REGISTERED_VAULT_PDA")]
    fn clone_project_cannot_supply_same_domain_checkpoint() {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        validate_receipt_pda(
            "11111111111111111111111111111111",
            &"09".repeat(32),
            b"checkpoint",
            [3; 32],
            &request([1; 32], b"checkpoint", [3; 32]),
        );
    }

    #[test]
    #[should_panic(expected = "RECEIPT_NOT_REGISTERED_VAULT_PDA")]
    fn clone_project_cannot_supply_same_domain_funding_receipt() {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        validate_receipt_pda(
            "11111111111111111111111111111111",
            &"09".repeat(32),
            b"funding",
            [3; 32],
            &request([1; 32], b"funding", [3; 32]),
        );
    }
}
