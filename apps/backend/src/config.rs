use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chains::near::GenesisIdentity;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Ethereum,
    Solana,
    Near,
}

impl Chain {
    pub const ALL: [Self; 3] = [Self::Ethereum, Self::Solana, Self::Near];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ethereum => "ethereum",
            Self::Solana => "solana",
            Self::Near => "near",
        }
    }
}

impl std::str::FromStr for Chain {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "ethereum" => Ok(Self::Ethereum),
            "solana" => Ok(Self::Solana),
            "near" => Ok(Self::Near),
            _ => anyhow::bail!("Unsupported chain"),
        }
    }
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Deserialize)]
pub struct Config {
    pub ethereum: EthereumConfig,
    pub solana: SolanaConfig,
    pub near: NearConfig,
    pub projects: Vec<ProjectConfig>,
    pub liquidity: LiquidityConfig,
}

#[derive(Clone, Deserialize)]
pub struct EthereumConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    pub genesis_hash: String,
}

#[derive(Clone, Deserialize)]
pub struct SolanaConfig {
    pub rpc_url: String,
    pub genesis_hash: String,
}

#[derive(Clone, Deserialize)]
pub struct NearConfig {
    pub rpc_url: String,
    pub genesis: GenesisIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectConfig {
    pub coordinator: String,
    pub project_id: String,
    pub ethereum_start_block: u64,
    pub domains: BTreeMap<Chain, DomainConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DomainConfig {
    pub domain: String,
    pub vault: String,
    pub asset: String,
    pub asset_id: String,
}

#[derive(Clone, Deserialize)]
pub struct LiquidityConfig {
    pub url: String,
    pub local_simulator: bool,
    pub api_key_file: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let config = Self::read(Path::new(
            &std::env::var("APODECT_CONFIG").context("APODECT_CONFIG is required")?,
        ))?;
        if std::env::var("APODECT_REQUIRE_PUBLIC_PROVIDERS").as_deref() == Ok("true") {
            ensure!(
                !config.liquidity.local_simulator,
                "Public deployment cannot use local provider simulators"
            );
        }
        Ok(config)
    }

    pub fn read(path: &Path) -> Result<Self> {
        let mut config: Self = serde_json::from_slice(
            &std::fs::read(path).context("Cannot read deployment configuration")?,
        )?;
        // Canonical identities prevent equivalent JSON spellings from registering
        // the same deployment twice or looking like a custody change.
        config.ethereum.genesis_hash =
            format!("0x{}", hex::encode(word(&config.ethereum.genesis_hash)?));
        for project in &mut config.projects {
            project.project_id = hex::encode(word(&project.project_id)?);
            project
                .coordinator
                .parse::<near_primitives::types::AccountId>()
                .context("Invalid coordinator account")?;
            for (chain, domain) in &mut project.domains {
                domain.domain = hex::encode(word(&domain.domain)?);
                if *chain == Chain::Ethereum {
                    domain.vault = domain
                        .vault
                        .parse::<alloy_primitives::Address>()?
                        .to_string();
                    domain.asset = domain
                        .asset
                        .parse::<alloy_primitives::Address>()?
                        .to_string();
                }
            }
        }
        ensure!(
            !config.projects.is_empty(),
            "Configure at least one Project"
        );
        let mut projects = BTreeSet::new();
        let mut domains = BTreeSet::new();
        for project in &config.projects {
            word(&project.project_id)?;
            ensure!(
                projects.insert(config.project_key(project)),
                "Duplicate Project configuration"
            );
            ensure!(
                project.domains.len() == 3,
                "Each Project must configure all three domains"
            );
            for chain in Chain::ALL {
                let domain = project
                    .domains
                    .get(&chain)
                    .context("Missing Project chain")?;
                word(&domain.domain)?;
                ensure!(
                    domains.insert(domain.domain.to_lowercase()),
                    "A domain belongs to only one Project"
                );
                ensure!(
                    !domain.vault.is_empty() && !domain.asset.is_empty(),
                    "Vault and asset are required"
                );
                ensure!(!domain.asset_id.is_empty(), "Provider asset ID is required");
            }
        }
        Ok(config)
    }

    pub fn project_key(&self, project: &ProjectConfig) -> String {
        let identity = serde_json::to_vec(&(
            &self.near.genesis,
            &project.coordinator,
            &project.project_id,
        ))
        .expect("deployment identity is serializable");
        hex::encode(Sha256::digest(identity))
    }

    pub fn network(&self, chain: Chain) -> String {
        match chain {
            Chain::Ethereum => self.ethereum.genesis_hash.clone(),
            Chain::Solana => self.solana.genesis_hash.clone(),
            Chain::Near => hex::encode(Sha256::digest(
                serde_json::to_vec(&self.near.genesis).expect("Genesis serializes"),
            )),
        }
    }
}

pub fn word(text: &str) -> Result<[u8; 32]> {
    hex::decode(text.strip_prefix("0x").unwrap_or(text))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Expected a 32-byte identity"))
}

pub fn number_word(number: u64) -> String {
    let mut bytes = [0; 32];
    bytes[24..].copy_from_slice(&number.to_be_bytes());
    hex::encode(bytes)
}
