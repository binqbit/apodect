//! Read-only deployment admission. This uses native clients without connecting
//! to PostgreSQL, loading relay credentials or submitting transactions.

use alloy_primitives::{Address, keccak256};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    chains::protocol::canonical_json,
    chains::{
        ethereum::Ethereum,
        near::{CoordinatorState, NearBlock, NearClient},
        solana::{Binding, Client as SolanaClient},
    },
    config::{Chain, Config, ProjectConfig, word},
    indexing::validate_domain_binding,
    liquidity::Liquidity,
    providers::inspect_mpc,
    rpc::Rpc,
};

#[derive(Serialize)]
pub struct DeploymentReport {
    pub mode: &'static str,
    pub ready: bool,
    pub checks: Vec<DeploymentCheck>,
    pub notes: Vec<String>,
}

#[derive(Serialize)]
pub struct DeploymentCheck {
    pub name: String,
    pub passed: bool,
    pub details: Value,
}

impl DeploymentReport {
    fn record(&mut self, name: String, result: Result<Value>) {
        let (passed, details) = match result {
            Ok(details) => (true, details),
            Err(error) => (false, json!({"error":format!("{error:#}")})),
        };
        self.ready &= passed;
        self.checks.push(DeploymentCheck {
            name,
            passed,
            details,
        });
    }
}

pub async fn inspect(config: &Config) -> Result<DeploymentReport> {
    let mut report = DeploymentReport {
        mode: if config.liquidity.local_simulator {
            "local"
        } else {
            "public"
        },
        ready: true,
        checks: Vec::new(),
        notes: vec![
            "Read-only admission checks do not prove MPC request liveness, operator finality or liquidity execution/refunds.".into(),
        ],
    };
    let ethereum = Ethereum::new(
        Rpc::new(&config.ethereum.rpc_url)?,
        config.ethereum.chain_id,
        config.ethereum.genesis_hash.parse()?,
    );
    report.record(
        "ethereum/network".into(),
        async {
            ethereum.verify_network().await?;
            let block = ethereum.finalized_block().await?;
            Ok(json!({"chain_id":config.ethereum.chain_id,"genesis_hash":config.ethereum.genesis_hash,"finalized":block}))
        }
        .await,
    );
    let near = NearClient::connect(Rpc::new(&config.near.rpc_url)?, &config.near.genesis).await;
    report.record(
        "near/network".into(),
        match &near {
            Ok(near) => near
                .finalized_block()
                .await
                .map(|block| json!({"chain_id":config.near.genesis.chain_id,"finalized":block})),
            Err(error) => Err(anyhow::anyhow!("{error:#}")),
        },
    );
    report.record(
        "liquidity/assets".into(),
        async {
            Liquidity::new(&config.liquidity)?
                .validate_assets(config)
                .await?;
            Ok(json!({"local_simulator":config.liquidity.local_simulator}))
        }
        .await,
    );

    for project in &config.projects {
        let prefix = format!("project/{}", config.project_key(project));
        // Network validation remains useful even when NEAR is unavailable.
        report.record(
            format!("{prefix}/solana/network-and-asset"),
            async {
                let client = solana_client(config, project)?;
                client.validate_deployment().await?;
                Ok(json!({"genesis_hash":config.solana.genesis_hash,"program":apodect_solana::ID.to_string()}))
            }
            .await,
        );
        let Ok(near) = &near else { continue };
        let coordinator = async {
            let block = near.finalized_block().await?;
            let state = near.coordinator_state(&project.coordinator, &block).await?;
            ensure!(
                word(&state.project_id)? == word(&project.project_id)?,
                "Coordinator Project identity mismatch"
            );
            signing_key(&state)?;
            Ok::<_, anyhow::Error>((block, state))
        }
        .await;
        let (block, state) = match coordinator {
            Ok(result) => result,
            Err(error) => {
                report.record(format!("{prefix}/coordinator"), Err(error));
                continue;
            }
        };
        report.record(
            format!("{prefix}/coordinator"),
            Ok(json!({"account":project.coordinator,"project_id":state.project_id,"block":block})),
        );
        for chain in Chain::ALL {
            if !config.liquidity.local_simulator {
                report.record(
                    format!("{prefix}/{chain}/domain"),
                    async {
                        let expected = public_domain(config, project, chain)?;
                        ensure!(
                            word(&project.domains[&chain].domain)? == word(&expected)?,
                            "Domain differs from its network/custody identity; expected {expected}"
                        );
                        Ok(json!({"domain":expected}))
                    }
                    .await,
                );
            }
            report.record(
                format!("{prefix}/{chain}/binding"),
                inspect_vault(config, project, chain, near, &ethereum, &block, &state).await,
            );
        }
        if !config.liquidity.local_simulator {
            match inspect_mpc(near, &state, &project.coordinator, &block).await {
                Ok(mpc) => {
                    report.ready &= mpc.issues.is_empty();
                    report.checks.push(DeploymentCheck {
                        name: format!("{prefix}/mpc"),
                        passed: mpc.issues.is_empty(),
                        details: serde_json::to_value(mpc)?,
                    });
                }
                Err(error) => report.record(format!("{prefix}/mpc"), Err(error)),
            }
        }
    }
    if config.liquidity.local_simulator {
        report.notes.push(
            "Local simulators were selected; public provider admission was not checked.".into(),
        );
    }
    Ok(report)
}

/// Public domains use canonical network identities. Anvil's local bootstrap
/// supplies its separate persistent instance marker to the same wire algorithm.
pub fn public_domain(config: &Config, project: &ProjectConfig, chain: Chain) -> Result<String> {
    let near_genesis = format!(
        "0x{}",
        hex::encode(Sha256::digest(canonical_json(&serde_json::to_value(
            &config.near.genesis
        )?)?))
    );
    let native_genesis = match chain {
        Chain::Ethereum => format!("0x{}", hex::encode(word(&config.ethereum.genesis_hash)?)),
        Chain::Solana => config.solana.genesis_hash.clone(),
        Chain::Near => near_genesis.clone(),
    };
    let binding = &project.domains[&chain];
    let (vault, asset) = match chain {
        Chain::Ethereum => (
            binding.vault.parse::<Address>()?.to_string(),
            binding.asset.parse::<Address>()?.to_string(),
        ),
        Chain::Solana => (
            binding
                .vault
                .parse::<anchor_lang::prelude::Pubkey>()?
                .to_string(),
            binding
                .asset
                .parse::<anchor_lang::prelude::Pubkey>()?
                .to_string(),
        ),
        Chain::Near => (
            binding
                .vault
                .parse::<near_primitives::types::AccountId>()?
                .to_string(),
            binding
                .asset
                .parse::<near_primitives::types::AccountId>()?
                .to_string(),
        ),
    };
    project
        .coordinator
        .parse::<near_primitives::types::AccountId>()?;
    let project_id = hex::encode(word(&project.project_id)?);
    let fields = [
        "apodect.coordination.v1",
        &project.coordinator,
        &near_genesis,
        &project_id,
        chain.as_str(),
        &native_genesis,
        &vault,
        &asset,
    ];
    ensure!(
        fields
            .iter()
            .all(|field| !field.is_empty() && !field.contains('\n')),
        "Invalid coordination domain component"
    );
    Ok(hex::encode(Sha256::digest(
        format!("{}\n", fields.join("\n")).as_bytes(),
    )))
}

/// Public processes must refuse incompatible bindings before they can prepare
/// or relay financial actions. Local startup retains its simulator workflow.
pub async fn verify_public(config: &Config) -> Result<()> {
    if config.liquidity.local_simulator {
        return Ok(());
    }
    let report = inspect(config).await?;
    if !report.ready {
        let failures = report
            .checks
            .iter()
            .filter(|check| !check.passed)
            .map(|check| format!("{}: {}", check.name, check.details))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!("Public deployment admission failed:\n{failures}");
    }
    Ok(())
}

fn solana_client(config: &Config, project: &ProjectConfig) -> Result<SolanaClient> {
    let binding = &project.domains[&Chain::Solana];
    SolanaClient::new(
        Rpc::new(&config.solana.rpc_url)?,
        Binding {
            genesis_hash: config.solana.genesis_hash.clone(),
            project: binding.vault.parse()?,
            mint: binding.asset.parse()?,
            domain: word(&binding.domain)?,
        },
        None,
    )
}

fn signing_key(state: &CoordinatorState) -> Result<[u8; 64]> {
    hex::decode(&state.sign_key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Coordinator signing key must contain 64 bytes"))
}

async fn inspect_vault(
    config: &Config,
    project: &ProjectConfig,
    chain: Chain,
    near: &NearClient,
    ethereum: &Ethereum,
    block: &NearBlock,
    state: &CoordinatorState,
) -> Result<Value> {
    let configured = &project.domains[&chain];
    let domain = near
        .coordinator_domain(&project.coordinator, &configured.domain, block)
        .await?;
    validate_domain_binding(chain, configured, &domain)?;
    let key = signing_key(state)?;
    let owner: [u8; 32] = match chain {
        Chain::Ethereum => {
            let finalized = ethereum.finalized_block().await?;
            ensure!(
                project.ethereum_start_block <= finalized.height,
                "Ethereum creation block is not finalized"
            );
            let vault: Address = configured.vault.parse()?;
            ethereum
                .verify_creation_block(vault, project.ethereum_start_block)
                .await?;
            let local = ethereum.project(vault, &finalized).await?;
            ensure!(
                local.domain.0 == word(&configured.domain)?
                    && local.token == configured.asset.parse::<Address>()?,
                "Ethereum vault binding differs from coordinator"
            );
            let expected = Address::from_slice(&keccak256(key)[12..]);
            ensure!(
                ethereum.coordinator_signer(vault, &finalized).await? == expected,
                "Ethereum vault command signer differs from coordinator"
            );
            let mut identity = [0; 32];
            identity[12..].copy_from_slice(local.owner.as_slice());
            identity
        }
        Chain::Solana => {
            let client = solana_client(config, project)?;
            let local = client.read_project().await?;
            ensure!(
                local.value.command_key == key,
                "Solana vault command key differs from coordinator"
            );
            let owner = client
                .read_workspace(local.value.workspace)
                .await?
                .value
                .owner
                .to_bytes();
            client
                .token_balance(
                    anchor_lang::prelude::Pubkey::new_from_array(word(&domain.owner_recipient)?),
                    anchor_lang::prelude::Pubkey::new_from_array(owner),
                )
                .await?
                .context("Solana owner withdrawal token account is not initialized")?;
            owner
        }
        Chain::Near => {
            let local = near.local_project(&configured.vault, block).await?;
            let bridge = near.local_bridge(&configured.vault, block).await?;
            ensure!(
                local.token_id == configured.asset
                    && bridge.domain.as_deref().map(word).transpose()?
                        == Some(word(&configured.domain)?),
                "NEAR vault binding differs from coordinator"
            );
            ensure!(
                bridge
                    .key
                    .as_deref()
                    .map(hex::decode)
                    .transpose()?
                    .as_deref()
                    == Some(key.as_slice()),
                "NEAR vault command key differs from coordinator"
            );
            let metadata: Value = near
                .view(&configured.asset, "ft_metadata", &json!({}), block)
                .await?;
            ensure!(
                metadata["decimals"] == 6,
                "NEAR asset must have six decimals"
            );
            let storage: Value = near
                .view(
                    &configured.asset,
                    "storage_balance_of",
                    &json!({"account_id":configured.vault}),
                    block,
                )
                .await?;
            ensure!(
                !storage.is_null(),
                "NEAR vault is not registered with its token"
            );
            Sha256::digest(local.owner_id.as_bytes()).into()
        }
    };
    ensure!(
        word(&domain.owner_identity)? == owner,
        "Coordinator owner identity differs from native vault owner"
    );
    if chain != Chain::Solana {
        ensure!(
            word(&domain.owner_recipient)? == owner,
            "Coordinator withdrawal recipient differs from native vault owner"
        );
    }
    Ok(
        json!({"domain":configured.domain,"vault":configured.vault,"asset":configured.asset,"command_key_verified":true,"owner_verified":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_domains_match_independent_protocol_vectors() {
        // Expected hashes come from scripts/crosschain/protocol.py, including
        // canonical NEAR genesis JSON, 0x prefixes and the final newline.
        let config: Config = serde_json::from_value(json!({
            "ethereum":{"rpc_url":"http://localhost","chain_id":1,"genesis_hash":format!("0x{}","00".repeat(32))},
            "solana":{"rpc_url":"http://localhost","genesis_hash":"11111111111111111111111111111111"},
            "near":{"rpc_url":"http://localhost","genesis":{
                "chain_id":"fixture","genesis_time":"2026-11-11T00:00:00Z","genesis_height":0,"validators":[]
            }},
            "projects":[{"name":"Test project","coordinator":"coordinator.near","project_id":"01".repeat(32),"ethereum_start_block":1,"domains":{
                "ethereum":{"domain":"unbound","vault":"0x1111111111111111111111111111111111111111","asset":"0x2222222222222222222222222222222222222222","asset_id":"eth"},
                "solana":{"domain":"unbound","vault":"11111111111111111111111111111111","asset":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","asset_id":"sol"},
                "near":{"domain":"unbound","vault":"vault.near","asset":"usdc.near","asset_id":"near"}
            }}],
            "liquidity":{"url":"https://provider.example","local_simulator":false,"api_key_file":null}
        })).unwrap();
        for (chain, expected) in [
            (
                Chain::Ethereum,
                "eca368de9b1cd79ba457f1c04c8ef851d09cb9217198b9c5136e63d858ab46a2",
            ),
            (
                Chain::Solana,
                "f98992e9a796336fc75261fa43a35d22ab09760febcef6d3ec4936a80a031237",
            ),
            (
                Chain::Near,
                "c0a9841d13a3c20188adf1ee5c9ebca479e906fcc06979f17bde43b6c2d49867",
            ),
        ] {
            assert_eq!(
                public_domain(&config, &config.projects[0], chain).unwrap(),
                expected
            );
            let mut changed = config.clone();
            changed.projects[0].coordinator = "other.near".into();
            assert_ne!(
                public_domain(&changed, &changed.projects[0], chain).unwrap(),
                expected
            );
            changed = config.clone();
            changed.projects[0].project_id = "02".repeat(32);
            assert_ne!(
                public_domain(&changed, &changed.projects[0], chain).unwrap(),
                expected
            );
        }
    }
}
