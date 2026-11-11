//! Read-only admission checks for the deployed NEAR MPC interface.
//!
//! Key views and advertised capabilities are checked at one finalized block.
//! They do not prove that operators will accept or complete a future request.

use anyhow::{Context, Result, ensure};
use near_crypto::PublicKey;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chains::near::{CoordinatorState, NearBlock, NearClient};

// The coordinator attaches this budget to both public MPC request methods.
const MPC_CALL_GAS_TGAS: u64 = 60;

#[derive(Debug, Serialize)]
pub struct MpcReport {
    pub provider_id: String,
    pub block: NearBlock,
    pub version: Option<String>,
    pub protocol_state: Option<String>,
    pub minimum_gas_tgas: Option<u64>,
    pub available_foreign_chains: Vec<String>,
    pub signing_key_verified: bool,
    pub read_key_verified: bool,
    pub issues: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
enum ProtocolState {
    Running(RunningState),
    Resharing {
        previous_running_state: RunningState,
    },
    #[serde(other)]
    Unavailable,
}

#[derive(Deserialize)]
struct RunningState {
    domains: DomainRegistry,
}

#[derive(Deserialize)]
struct DomainRegistry {
    domains: Vec<Domain>,
}

#[derive(Deserialize)]
struct Domain {
    id: u64,
    protocol: String,
    purpose: String,
}

#[derive(Deserialize)]
struct ProviderConfig {
    sign_call_gas_attachment_requirement_tera_gas: u64,
}

/// Compare the coordinator's immutable keys with public MPC views and capabilities.
/// Each independent read contributes its result so an unavailable chain does not
/// conceal another deployment error. This function never sends a transaction.
pub async fn inspect_mpc(
    near: &NearClient,
    state: &CoordinatorState,
    coordinator: &str,
    block: &NearBlock,
) -> Result<MpcReport> {
    coordinator
        .parse::<near_primitives::types::AccountId>()
        .context("Invalid coordinator account")?;
    state
        .mpc_id
        .parse::<near_primitives::types::AccountId>()
        .context("Invalid MPC provider account")?;
    ensure!(
        !state.sign_path.is_empty(),
        "Coordinator signing path is empty"
    );

    let empty = json!({});
    let signing_args = json!({"domain_id": 0, "path": state.sign_path, "predecessor": coordinator});
    let read_args = json!({"domain_id": 3});
    let (version, protocol, config, available, signing_key, read_key) = tokio::join!(
        near.view::<String>(&state.mpc_id, "version", &empty, block),
        near.view::<ProtocolState>(&state.mpc_id, "state", &empty, block),
        near.view::<ProviderConfig>(&state.mpc_id, "config", &empty, block),
        near.view::<Vec<String>>(&state.mpc_id, "get_available_foreign_chains", &empty, block),
        near.view::<String>(&state.mpc_id, "derived_public_key", &signing_args, block),
        near.view::<String>(&state.mpc_id, "public_key", &read_args, block),
    );

    let mut report = MpcReport {
        provider_id: state.mpc_id.clone(),
        block: block.clone(),
        version: None,
        protocol_state: None,
        minimum_gas_tgas: None,
        available_foreign_chains: Vec::new(),
        signing_key_verified: false,
        read_key_verified: false,
        issues: Vec::new(),
        notes: vec![
            "Read-only views cannot prove request admission or operator liveness; the MPC 3.18 API does not expose accept_requests.".into(),
            "Advertised chain support does not prove the operators' RPC network or finality configuration.".into(),
        ],
    };

    if let Some(version) = observed(version, "version", &mut report.issues) {
        if version != "3.18.0" {
            report.notes.push(format!(
                "MPC version {version} differs from the reviewed 3.18.0 interface; capability checks are still applied."
            ));
        }
        report.version = Some(version);
    }
    if let Some(protocol) = observed(protocol, "state", &mut report.issues) {
        validate_domains(protocol, &mut report);
    }
    if let Some(config) = observed(config, "config", &mut report.issues) {
        let minimum = config.sign_call_gas_attachment_requirement_tera_gas;
        report.minimum_gas_tgas = Some(minimum);
        if minimum == 0 || minimum > MPC_CALL_GAS_TGAS {
            report.issues.push(format!(
                "MPC requires {minimum} Tgas; the coordinator's {MPC_CALL_GAS_TGAS} Tgas request budget is incompatible."
            ));
        }
    }
    if let Some(chains) = observed(
        available,
        "get_available_foreign_chains",
        &mut report.issues,
    ) {
        validate_chains(chains, &mut report);
    }
    if let Some(key) = observed(signing_key, "derived_public_key", &mut report.issues) {
        report.signing_key_verified = check_key(&key, &state.sign_key)
            .inspect_err(|error| {
                report
                    .issues
                    .push(format!("Coordinator signing key: {error}"));
            })
            .is_ok();
    }
    if let Some(key) = observed(read_key, "public_key", &mut report.issues) {
        report.read_key_verified = check_key(&key, &state.read_key)
            .inspect_err(|error| {
                report.issues.push(format!("Read MPC root key: {error}"));
            })
            .is_ok();
    }
    Ok(report)
}

fn observed<T>(result: Result<T>, method: &str, issues: &mut Vec<String>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(format!("Cannot read MPC {method}: {error}"));
            None
        }
    }
}

fn validate_domains(protocol: ProtocolState, report: &mut MpcReport) {
    let (name, active) = match protocol {
        ProtocolState::Running(state) => ("Running", state),
        ProtocolState::Resharing {
            previous_running_state,
        } => ("Resharing", previous_running_state),
        ProtocolState::Unavailable => {
            report
                .issues
                .push("MPC is not Running or Resharing.".into());
            return;
        }
    };
    report.protocol_state = Some(name.into());
    for (id, purpose) in [(0, "Sign"), (3, "ForeignTx")] {
        let mut matching = active
            .domains
            .domains
            .iter()
            .filter(|domain| domain.id == id);
        let valid = matching.next().is_some_and(|domain| {
            domain.purpose == purpose
                && matches!(domain.protocol.as_str(), "CaitSith" | "RobustEcdsa")
        }) && matching.next().is_none();
        if !valid {
            report.issues.push(format!(
                "MPC domain {id} must be an active Secp256k1 domain with purpose {purpose}."
            ));
        }
    }
}

fn validate_chains(mut chains: Vec<String>, report: &mut MpcReport) {
    chains.sort();
    chains.dedup();
    for required in ["Ethereum", "Solana"] {
        if !chains.iter().any(|chain| chain == required) {
            report.issues.push(format!(
                "MPC does not advertise {required} foreign verification; the agreed three-chain evidence path is unavailable and cannot be replaced by trusted backend observations."
            ));
        }
    }
    report.available_foreign_chains = chains;
}

fn check_key(public: &str, expected_hex: &str) -> Result<()> {
    let key: PublicKey = public.parse().context("Invalid provider public key")?;
    ensure!(
        matches!(key, PublicKey::SECP256K1(_)),
        "Provider public key is not Secp256k1"
    );
    let mut point = [0; 65];
    point[0] = 4;
    point[1..].copy_from_slice(key.key_data());
    alloy_signer::k256::PublicKey::from_sec1_bytes(&point)
        .context("Provider public key is not a valid Secp256k1 point")?;
    let expected = hex::decode(expected_hex.strip_prefix("0x").unwrap_or(expected_hex))
        .context("Invalid coordinator public key encoding")?;
    ensure!(
        expected.len() == 64,
        "Coordinator public key must contain 64 bytes"
    );
    ensure!(
        key.key_data() == expected,
        "Provider public key does not match the coordinator's pinned key"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> MpcReport {
        MpcReport {
            provider_id: "v1.signer".into(),
            block: NearBlock {
                height: 1,
                hash: "fixture".into(),
            },
            version: None,
            protocol_state: None,
            minimum_gas_tgas: None,
            available_foreign_chains: Vec::new(),
            signing_key_verified: false,
            read_key_verified: false,
            issues: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn public_key_verification_rejects_different_keys_and_curves() {
        let key = hex::decode(concat!(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            "483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8"
        ))
        .unwrap();
        let public = format!("secp256k1:{}", bs58::encode(&key).into_string());
        check_key(&public, &hex::encode(&key)).unwrap();
        check_key(&public, &format!("0x{}", hex::encode(&key))).unwrap();
        assert!(check_key(&public, &hex::encode([8; 64])).is_err());
        assert!(check_key(&public, "00").is_err());
        let ed25519 = format!("ed25519:{}", bs58::encode([7; 32]).into_string());
        assert!(check_key(&ed25519, &hex::encode(&key)).is_err());
        let invalid = format!("secp256k1:{}", bs58::encode([7; 64]).into_string());
        assert!(check_key(&invalid, &hex::encode([7; 64])).is_err());
    }

    #[test]
    fn admission_requires_correct_purposes_in_active_domains() {
        let domains = json!({"domains":{"domains":[
            {"id":0,"protocol":"CaitSith","purpose":"Sign"},
            {"id":3,"protocol":"CaitSith","purpose":"ForeignTx"}
        ]}});
        for state in [
            json!({"Running":domains}),
            json!({"Resharing":{"previous_running_state":domains}}),
        ] {
            let mut result = report();
            validate_domains(serde_json::from_value(state).unwrap(), &mut result);
            assert!(result.issues.is_empty());
        }
        let mut invalid = domains;
        invalid["domains"]["domains"][1]["purpose"] = json!("Sign");
        let mut result = report();
        validate_domains(
            serde_json::from_value(json!({"Running":invalid})).unwrap(),
            &mut result,
        );
        assert_eq!(result.issues.len(), 1);
        assert!(result.issues[0].contains("domain 3"));
    }

    #[test]
    fn missing_ethereum_admission_is_not_hidden_by_solana_support() {
        let mut result = report();
        validate_chains(
            vec!["Solana".into(), "Solana".into(), "Base".into()],
            &mut result,
        );
        assert_eq!(result.available_foreign_chains, ["Base", "Solana"]);
        assert_eq!(result.issues.len(), 1);
        assert!(result.issues[0].contains("Ethereum"));
        assert!(result.issues[0].contains("trusted backend"));
    }
}
