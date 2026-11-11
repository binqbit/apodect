//! Host-side interpretation of the contract's fixed wire records.
//! Native adapters enforce their chain's identities; NEAR's numeric task zero is valid.

use alloy_primitives::{B256, Signature, U256, uint};
use anyhow::{Context, Result, ensure};
use apodect_solana::bridge::{Command, integer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::word;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Selection {
    pub task: Option<String>,
    pub request: Option<String>,
    pub plan: Option<String>,
    pub route: Option<String>,
    pub operation: Option<String>,
    pub refund: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
    pub payload: String,
    pub request: Option<Value>,
    pub values: Option<Value>,
}

pub struct Checkpoint {
    pub domain: String,
    pub observation: String,
    pub sequence: u64,
    pub command_state: u64,
}

impl Checkpoint {
    pub fn parse(payload: &str) -> Result<Self> {
        let bytes = hex::decode(payload.strip_prefix("0x").unwrap_or(payload))?;
        ensure!(bytes.len() == 1248, "Checkpoint must contain 39 words");
        let (words, _) = bytes.as_chunks::<32>();
        let identities = [0, 1, 7, 16, 17, 18, 22, 23, 26, 27, 32, 33, 36, 37];
        for (index, value) in words.iter().enumerate() {
            if !identities.contains(&index) {
                integer(value).context("Noncanonical checkpoint integer")?;
            }
        }
        for index in [9, 10, 38] {
            ensure!(
                integer(&words[index])? <= 1,
                "Noncanonical checkpoint boolean"
            );
        }
        Ok(Self {
            domain: hex::encode(words[0]),
            observation: hex::encode(words[1]),
            sequence: integer(&words[2])?,
            command_state: integer(&words[35])?,
        })
    }
}

pub fn command_bytes(encoded: &str) -> Result<[u8; 384]> {
    hex::decode(encoded.strip_prefix("0x").unwrap_or(encoded))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Command must contain 12 words"))
}

pub fn parse_command(encoded: &str) -> Result<Command> {
    let bytes = command_bytes(encoded)?;
    let (words, _) = bytes.as_chunks::<32>();
    let action = u8::try_from(integer(&words[2])?)?;
    ensure!(
        (1..=15).contains(&action) && words[3] != [0; 32],
        "Invalid command action or operation"
    );
    let allowed: &[usize] = match action {
        1 => &[4, 7],
        2 | 3 => &[4, 7, 11],
        4 => &[4, 7, 9, 11],
        5 => &[4, 5, 6, 7],
        6 => &[4],
        7 => &[5, 6],
        8 => &[5],
        9 | 11 | 12 => &[1, 5],
        10 => &[1, 5, 7],
        13 => &[5, 7, 11],
        14 => &[7, 9],
        15 => &[11],
        _ => unreachable!(),
    };
    for index in [1, 4, 5, 6, 7, 8, 9, 11] {
        ensure!(
            allowed.contains(&index) || words[index] == [0; 32],
            "Nonzero reserved command word"
        );
    }
    let command = Command {
        domain: words[0],
        nonce: integer(&words[1])?,
        action,
        operation: words[3],
        related: words[4],
        task: words[5],
        request: words[6],
        amount: integer(&words[7])?,
        recipient: words[9],
        expires: integer(&words[10])?,
        reference: words[11],
    };
    ensure!(
        action > 6 || command.related != [0; 32],
        "Command plan is zero"
    );
    ensure!(
        !matches!(action, 2..=5 | 10 | 13 | 14) || command.amount > 0,
        "Command amount is zero"
    );
    ensure!(
        !matches!(action, 2..=4 | 13 | 15) || command.reference != [0; 32],
        "Command reference is zero"
    );
    ensure!(
        !matches!(action, 4 | 14) || command.recipient != [0; 32],
        "Command recipient is zero"
    );
    Ok(command)
}

/// Matches the protocol's compact, sorted ASCII JSON quote commitment.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    let mut value = value.clone();
    value.sort_all_objects();
    let encoded = serde_json::to_string(&value)?;
    let mut canonical = Vec::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_ascii() {
            canonical.push(character as u8);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]) {
                canonical.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
            }
        }
    }
    Ok(canonical)
}

pub fn mpc_signature(response: &Value, digest: &str, key: &str) -> Result<([u8; 64], u8)> {
    ensure!(
        response["scheme"] == "Secp256k1",
        "Unexpected MPC signature scheme"
    );
    let point = hex::decode(
        response["big_r"]["affine_point"]
            .as_str()
            .context("Missing MPC R point")?,
    )?;
    let scalar = word(
        response["s"]["scalar"]
            .as_str()
            .context("Missing MPC scalar")?,
    )?;
    let parity = response["recovery_id"]
        .as_u64()
        .context("Missing MPC recovery ID")?;
    ensure!(
        point.len() == 33 && parity <= 1 && point[0] == 2 + parity as u8,
        "Invalid MPC recovery point"
    );
    let signature = Signature::from_scalars_and_parity(
        B256::from_slice(&point[1..]),
        scalar.into(),
        parity == 1,
    );
    ensure!(
        signature.s() != U256::ZERO
            && signature.s()
                <= uint!(0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0_U256),
        "Noncanonical MPC signature scalar"
    );
    let public = signature.recover_from_prehash(&word(digest)?.into())?;
    let expected = hex::decode(key.strip_prefix("0x").unwrap_or(key))?;
    ensure!(
        public.to_encoded_point(false).as_bytes()[1..] == expected,
        "MPC signing key mismatch"
    );
    let mut compact = [0; 64];
    compact[..32].copy_from_slice(&point[1..]);
    compact[32..].copy_from_slice(&scalar);
    Ok((compact, parity as u8))
}

pub fn identity(parts: &[&str]) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(parts).expect("strings serialize"),
    ))
}

pub fn word_number(encoded: &str) -> Result<u64> {
    integer(&word(encoded)?).context("Identity exceeds native u64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn existing_contract_signature_authenticates_each_command_word() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../../contracts/near/project/tests/fixtures/coordinated-signatures.json"
        ))
        .unwrap();
        let fixture = &fixtures["reserve"];
        let wire = command_bytes(fixture["command"].as_str().unwrap()).unwrap();
        let compact = fixture["signature"].as_str().unwrap();
        let key = fixtures["public_key"].as_str().unwrap();
        let allocation = parse_command(fixtures["allocate"]["command"].as_str().unwrap()).unwrap();
        assert_eq!(allocation.task, [0; 32]);
        assert_eq!(allocation.request, [0; 32]);
        let digest = hex::encode(Sha256::digest(wire));
        let response = json!({"scheme": "Secp256k1", "recovery_id": 1,
            "big_r": {"affine_point": format!("03{}", &compact[..64])},
            "s": {"scalar": &compact[64..]}});
        assert_eq!(
            hex::encode(mpc_signature(&response, &digest, key).unwrap().0),
            compact
        );
        for index in 0..12 {
            let mut changed = wire;
            changed[index * 32 + 31] ^= 1;
            assert!(mpc_signature(&response, &hex::encode(Sha256::digest(changed)), key).is_err());
        }
        for scalar in ["00".repeat(32), "ff".repeat(32)] {
            let mut changed = response.clone();
            changed["s"]["scalar"] = scalar.into();
            assert!(mpc_signature(&changed, &digest, key).is_err());
        }
        assert!(mpc_signature(&response, &digest, &"00".repeat(64)).is_err());
    }
}
