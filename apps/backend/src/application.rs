use std::sync::Arc;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::{
    accounts::{Accounts, AuthConfig},
    config::Config,
    database,
    indexing::Indexer,
    liquidity::Liquidity,
    rpc::Rpc,
    storage::Store,
};

/// Shared services for the HTTP process and operational CLI. This composition
/// contains no signing keys; the worker loads its separate relay credentials.
pub struct Application {
    pub indexer: Arc<Indexer>,
    pub accounts: Accounts,
    pub liquidity: Liquidity,
}

impl Application {
    pub async fn connect() -> Result<Self> {
        let config = Config::from_env()?;
        let pool = database::connect().await.map_err(anyhow::Error::msg)?;
        let store = Store::new(pool.clone());
        store.migrate().await?;
        let indexer = Arc::new(Indexer::connect(config, store).await?);
        let config = &indexer.config;
        let accounts = Accounts::new(
            pool,
            AuthConfig {
                origin: std::env::var("APP_ORIGIN")
                    .context("APP_ORIGIN is required for wallet authentication")?,
                ethereum_chain_id: config.ethereum.chain_id,
                ethereum_genesis_hash: config.ethereum.genesis_hash.clone(),
                solana_cluster: std::env::var("APODECT_SOLANA_CLUSTER")
                    .or_else(|error| {
                        if config.liquidity.local_simulator {
                            Ok("localnet".to_owned())
                        } else {
                            Err(error)
                        }
                    })
                    .context("APODECT_SOLANA_CLUSTER is required")?,
                solana_genesis_hash: config.solana.genesis_hash.clone(),
                near_network: hex::encode(Sha256::digest(serde_json::to_vec(
                    &config.near.genesis,
                )?)),
            },
            Rpc::new(&config.ethereum.rpc_url)?,
            indexer.near.clone(),
        )?;
        let liquidity = Liquidity::new(&config.liquidity)?;
        Ok(Self {
            indexer,
            accounts,
            liquidity,
        })
    }
}
