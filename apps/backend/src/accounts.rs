//! Wallet ownership establishes application identity, never on-chain spending rights.

use std::sync::Arc;

use alloy_primitives::{Address, Signature, eip191_hash_message};
use alloy_sol_types::{SolCall, sol};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use ed25519_dalek::{Signature as Ed25519Signature, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{chains::near::NearClient, config::Chain, rpc::Rpc};

sol! {
    function isValidSignature(bytes32 hash, bytes signature) external view returns (bytes4);
}

const CHALLENGE_MINUTES: i64 = 5;
const SESSION_HOURS: i64 = 24;

pub struct AuthConfig {
    pub origin: String,
    pub ethereum_chain_id: u64,
    pub ethereum_genesis_hash: String,
    pub solana_cluster: String,
    pub solana_genesis_hash: String,
    /// Stable chain/genesis identity from the verified deployment configuration.
    pub near_network: String,
}

#[derive(Clone)]
pub struct Accounts {
    pool: PgPool,
    config: Arc<AuthConfig>,
    domain: String,
    ethereum: Rpc,
    near: NearClient,
}

#[derive(Serialize)]
pub struct Challenge {
    pub id: Uuid,
    pub chain: Chain,
    pub address: String,
    pub message: String,
    /// NEP-413 takes these exact bytes; SIWE/SIWS use their hexadecimal encoding.
    pub nonce: [u8; 32],
    pub recipient: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct Profile {
    pub id: Uuid,
    pub display_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Serialize)]
pub struct ProfileLookup {
    pub profile: Profile,
    pub wallets: Vec<Wallet>,
}

#[derive(Debug)]
pub enum ProfileError {
    InvalidName,
    InvalidUsername,
    UsernameTaken,
}

impl ProfileError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidName => "Enter a name with 1 to 100 printable characters.",
            Self::InvalidUsername => {
                "Use 3 to 32 letters, numbers or underscores for your username, starting with a letter."
            }
            Self::UsernameTaken => "This username is already taken. Choose another one.",
        }
    }
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ProfileError {}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct Wallet {
    pub chain: String,
    pub network: String,
    pub address: String,
}

/// The bearer token is returned once; only its digest is persisted.
#[derive(Serialize)]
pub struct Login {
    pub session_token: String,
    pub expires_at: DateTime<Utc>,
    pub profile: Profile,
}

#[derive(Clone, Debug, Serialize)]
pub struct Session {
    pub profile: Profile,
    pub wallet: Wallet,
    pub expires_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ChallengeRow {
    id: Uuid,
    chain: String,
    network: String,
    address: String,
    nonce: Vec<u8>,
    message: String,
    link_session: Option<Vec<u8>>,
    callback_url: Option<String>,
}

#[derive(FromRow)]
struct SessionRow {
    profile_id: Uuid,
    display_name: Option<String>,
    username: Option<String>,
    chain: String,
    network: String,
    address: String,
    proof: Value,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SessionProof {
    Key,
    Near { public_key: String },
    EthereumContract { message: String, signature: String },
}

impl Accounts {
    pub fn new(
        pool: PgPool,
        mut config: AuthConfig,
        ethereum: Rpc,
        near: NearClient,
    ) -> Result<Self> {
        let origin =
            reqwest::Url::parse(&config.origin).context("invalid authentication origin")?;
        ensure!(
            matches!(origin.scheme(), "http" | "https")
                && origin.host_str().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.query().is_none()
                && origin.fragment().is_none()
                && origin.path() == "/",
            "authentication origin must contain only scheme, host and port"
        );
        ensure!(
            origin.scheme() == "https"
                || matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
            "nonlocal authentication origin requires HTTPS"
        );
        ensure!(
            ["mainnet", "testnet", "devnet", "localnet"].contains(&config.solana_cluster.as_str()),
            "invalid Solana sign-in cluster"
        );
        ensure!(
            !config.near_network.is_empty() && !config.near_network.contains(char::is_whitespace),
            "invalid NEAR authentication network"
        );
        ensure!(
            config.ethereum_chain_id > 0,
            "Ethereum chain ID must be positive"
        );
        let genesis = hex::decode(config.ethereum_genesis_hash.trim_start_matches("0x"))?;
        ensure!(genesis.len() == 32, "invalid Ethereum genesis hash");
        config.ethereum_genesis_hash = format!("0x{}", hex::encode(genesis));
        ensure!(
            bs58::decode(&config.solana_genesis_hash).into_vec()?.len() == 32,
            "invalid Solana genesis hash"
        );
        let domain = origin.authority().to_owned();
        config.origin = origin.origin().ascii_serialization();
        Ok(Self {
            pool,
            config: Arc::new(config),
            domain,
            ethereum,
            near,
        })
    }

    pub async fn begin_login(&self, chain: Chain, address: &str) -> Result<Challenge> {
        self.begin_login_with_callback(chain, address, None).await
    }

    pub async fn begin_login_with_callback(
        &self,
        chain: Chain,
        address: &str,
        callback_url: Option<&str>,
    ) -> Result<Challenge> {
        self.begin_challenge(chain, address, None, callback_url)
            .await
    }

    pub async fn begin_link(
        &self,
        session_token: &str,
        chain: Chain,
        address: &str,
    ) -> Result<Challenge> {
        self.begin_link_with_callback(session_token, chain, address, None)
            .await
    }

    pub async fn begin_link_with_callback(
        &self,
        session_token: &str,
        chain: Chain,
        address: &str,
        callback_url: Option<&str>,
    ) -> Result<Challenge> {
        self.session(session_token).await?;
        self.begin_challenge(
            chain,
            address,
            Some(token_hash(session_token)?),
            callback_url,
        )
        .await
    }

    async fn begin_challenge(
        &self,
        chain: Chain,
        address: &str,
        link_session: Option<Vec<u8>>,
        callback_url: Option<&str>,
    ) -> Result<Challenge> {
        if let Some(callback) = callback_url {
            ensure!(chain == Chain::Near, "only NEAR uses a signing callback");
            ensure!(
                callback == format!("{}/wallet/callback", self.config.origin),
                "signing callback must use the application's wallet callback URL"
            );
        }
        let address = normalize_address(chain, address)?;
        let network = self.network(chain);
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?;
        let expires_at = now + Duration::minutes(CHALLENGE_MINUTES);
        let nonce = random_bytes()?;
        let id = Uuid::new_v4();
        let statement = if link_session.is_some() {
            "Link this wallet to your Apodect profile."
        } else {
            "Sign in to Apodect."
        };
        let message = self.sign_in_message(chain, &address, &nonce, statement, now, id);

        sqlx::query("INSERT INTO login_challenges(id,chain,network,address,nonce,message,expires_at,link_session,callback_url) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(id).bind(chain.as_str()).bind(network).bind(&address).bind(nonce.as_slice())
            .bind(&message).bind(expires_at).bind(link_session).bind(callback_url).execute(&self.pool).await?;
        Ok(Challenge {
            id,
            chain,
            address,
            message,
            nonce,
            recipient: self.domain.clone(),
            callback_url: callback_url.map(str::to_owned),
            expires_at,
        })
    }

    pub async fn complete_login(
        &self,
        challenge_id: Uuid,
        signature: &str,
        public_key: Option<&str>,
    ) -> Result<Login> {
        let challenge = self.load_challenge(challenge_id).await?;
        ensure!(
            challenge.link_session.is_none(),
            "link challenge cannot create a login session"
        );
        let proof = self
            .verify_challenge(&challenge, signature, public_key)
            .await?;
        let token = hex::encode(random_bytes()?);
        let digest = token_hash(&token)?;

        let mut transaction = self.pool.begin().await?;
        consume_challenge(&mut transaction, challenge.id, None).await?;
        lock_wallet(&mut transaction, &challenge).await?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT profile_id FROM wallets WHERE chain=$1 AND network=$2 AND address=$3",
        )
        .bind(&challenge.chain)
        .bind(&challenge.network)
        .bind(&challenge.address)
        .fetch_optional(&mut *transaction)
        .await?;
        let profile_id = match existing {
            Some(profile_id) => profile_id,
            None => {
                let id = Uuid::new_v4();
                sqlx::query("INSERT INTO profiles(id) VALUES($1)")
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?;
                id
            }
        };
        save_wallet(&mut transaction, &challenge, profile_id, &proof).await?;
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO sessions(token_hash,chain,network,address,proof,expires_at)
            VALUES($1,$2,$3,$4,$5,clock_timestamp()+make_interval(hours=>$6))
            RETURNING expires_at
            "#,
        )
        .bind(digest)
        .bind(&challenge.chain)
        .bind(&challenge.network)
        .bind(&challenge.address)
        .bind(serde_json::to_value(proof)?)
        .bind(SESSION_HOURS as i32)
        .fetch_one(&mut *transaction)
        .await?;
        let profile = read_profile(&mut transaction, profile_id).await?;
        transaction.commit().await?;
        Ok(Login {
            session_token: token,
            expires_at,
            profile,
        })
    }

    pub async fn complete_link(
        &self,
        session_token: &str,
        challenge_id: Uuid,
        signature: &str,
        public_key: Option<&str>,
    ) -> Result<Profile> {
        let digest = token_hash(session_token)?;
        let session = self.session(session_token).await?;
        let challenge = self.load_challenge(challenge_id).await?;
        ensure!(
            challenge.link_session.as_ref() == Some(&digest),
            "link challenge belongs to another session"
        );
        let proof = self
            .verify_challenge(&challenge, signature, public_key)
            .await?;

        let mut transaction = self.pool.begin().await?;
        // Match logout's lock order: session first, then its dependent challenge.
        lock_session(&mut transaction, &digest).await?;
        consume_challenge(&mut transaction, challenge.id, Some(&digest)).await?;
        lock_wallet(&mut transaction, &challenge).await?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT profile_id FROM wallets WHERE chain=$1 AND network=$2 AND address=$3",
        )
        .bind(&challenge.chain)
        .bind(&challenge.network)
        .bind(&challenge.address)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(id) = existing {
            ensure!(
                id == session.profile.id,
                "wallet already belongs to another profile"
            );
        }
        save_wallet(&mut transaction, &challenge, session.profile.id, &proof).await?;
        let profile = read_profile(&mut transaction, session.profile.id).await?;
        transaction.commit().await?;
        Ok(profile)
    }

    pub async fn session(&self, session_token: &str) -> Result<Session> {
        let digest = token_hash(session_token)?;
        let row: SessionRow = sqlx::query_as(
            r#"
            SELECT w.profile_id,p.display_name,p.username,s.chain,s.network,s.address,s.proof,s.expires_at
            FROM sessions s
            JOIN wallets w USING(chain,network,address)
            JOIN profiles p ON p.id=w.profile_id
            WHERE s.token_hash=$1 AND s.expires_at>clock_timestamp()
            "#,
        )
        .bind(&digest)
        .fetch_optional(&self.pool)
        .await?
        .context("session is absent or expired")?;
        let chain = row.chain.parse()?;
        ensure!(
            row.network == self.network(chain),
            "session belongs to a different network"
        );
        match serde_json::from_value(row.proof)? {
            SessionProof::Key => {}
            SessionProof::Near { public_key } => {
                self.verify_near_access(&row.address, &public_key).await?
            }
            SessionProof::EthereumContract { message, signature } => {
                self.verify_contract_signature(&row.address, &message, &signature)
                    .await?
            }
        }
        // RPC verification can outlast a concurrent logout or expiration.
        let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE token_hash=$1 AND expires_at>clock_timestamp())")
            .bind(digest).fetch_one(&self.pool).await?;
        ensure!(active, "session is absent or expired");
        Ok(Session {
            profile: Profile {
                id: row.profile_id,
                display_name: row.display_name,
                username: row.username,
            },
            wallet: Wallet {
                chain: row.chain,
                network: row.network,
                address: row.address,
            },
            expires_at: row.expires_at,
        })
    }

    pub async fn revoke_session(&self, session_token: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
            .bind(token_hash(session_token)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn wallets(&self, session_token: &str) -> Result<Vec<Wallet>> {
        let session = self.session(session_token).await?;
        Ok(sqlx::query_as("SELECT chain,network,address FROM wallets WHERE profile_id=$1 ORDER BY chain,network,address")
            .bind(session.profile.id).fetch_all(&self.pool).await?)
    }

    /// Private management reads need current ownership, even when the profile
    /// signed in through a different linked wallet. A historic link alone is not
    /// evidence that a NEAR key still controls its account.
    pub async fn has_current_near_wallet(
        &self,
        session_token: &str,
        address: &str,
    ) -> Result<bool> {
        let session = self.session(session_token).await?;
        let address = normalize_address(Chain::Near, address)?;
        let public_key: Option<String> = sqlx::query_scalar(
            "SELECT near_public_key FROM wallets WHERE profile_id=$1 AND chain='near' AND network=$2 AND address=$3",
        )
        .bind(session.profile.id)
        .bind(self.network(Chain::Near))
        .bind(&address)
        .fetch_optional(&self.pool)
        .await?;
        match public_key {
            Some(public_key) => self.near_key_is_current(&address, &public_key).await,
            None => Ok(false),
        }
    }

    /// Select a verified wallet on this deployment's network for an unsigned action.
    pub async fn require_wallet(
        &self,
        session_token: &str,
        chain: Chain,
        address: &str,
    ) -> Result<Wallet> {
        let session = self.session(session_token).await?;
        let address = normalize_address(chain, address)?;
        sqlx::query_as("SELECT chain,network,address FROM wallets WHERE profile_id=$1 AND chain=$2 AND network=$3 AND address=$4")
            .bind(session.profile.id)
            .bind(chain.as_str())
            .bind(self.network(chain))
            .bind(address)
            .fetch_optional(&self.pool)
            .await?
            .context("actor wallet is not linked to this profile on the configured network")
    }

    pub async fn update_profile(
        &self,
        session_token: &str,
        name: Option<&str>,
        username: Option<&str>,
    ) -> Result<Profile> {
        let session = self.session(session_token).await?;
        let name = name.map(str::trim);
        if let Some(name) = name {
            ensure!(
                !name.is_empty()
                    && name.chars().count() <= 100
                    && !name.chars().any(char::is_control),
                ProfileError::InvalidName
            );
        }
        let username = username.map(normalize_username).transpose()?;
        let mut transaction = self.pool.begin().await?;
        lock_session(&mut transaction, &token_hash(session_token)?).await?;
        let profile = sqlx::query_as(
            "UPDATE profiles SET display_name=$2,username=$3 WHERE id=$1 RETURNING id,display_name,username",
        )
        .bind(session.profile.id)
        .bind(name)
        .bind(username)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| {
            if error.as_database_error().is_some_and(|error| {
                error.is_unique_violation() && error.constraint() == Some("profiles_username_key")
            }) {
                anyhow::Error::new(ProfileError::UsernameTaken)
            } else {
                error.into()
            }
        })?;
        transaction.commit().await?;
        Ok(profile)
    }

    /// A username opts into exact lookup; it never grants project permissions.
    pub async fn profile_by_username(
        &self,
        session_token: &str,
        username: &str,
    ) -> Result<Option<ProfileLookup>> {
        self.session(session_token).await?;
        let username = normalize_username(username)?;
        let profile: Option<Profile> =
            sqlx::query_as("SELECT id,display_name,username FROM profiles WHERE username=$1")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        let Some(profile) = profile else {
            return Ok(None);
        };
        let records: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT chain,network,address,near_public_key FROM wallets WHERE profile_id=$1 \
             AND ((chain='ethereum' AND network=$2) OR (chain='solana' AND network=$3) \
             OR (chain='near' AND network=$4)) ORDER BY chain,address",
        )
        .bind(profile.id)
        .bind(self.network(Chain::Ethereum))
        .bind(self.network(Chain::Solana))
        .bind(self.network(Chain::Near))
        .fetch_all(&self.pool)
        .await?;
        let mut wallets = Vec::with_capacity(records.len());
        for (chain, network, address, near_public_key) in records {
            if let Some(public_key) = near_public_key
                && !self.near_key_is_current(&address, &public_key).await?
            {
                continue;
            }
            wallets.push(Wallet {
                chain,
                network,
                address,
            });
        }
        Ok(Some(ProfileLookup { profile, wallets }))
    }

    pub async fn purge_expired(pool: &PgPool) -> Result<()> {
        let mut transaction = pool.begin().await?;
        sqlx::query("DELETE FROM sessions WHERE expires_at<=clock_timestamp()")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM login_challenges WHERE expires_at<=clock_timestamp()")
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn load_challenge(&self, id: Uuid) -> Result<ChallengeRow> {
        let row: ChallengeRow = sqlx::query_as(
            "SELECT * FROM login_challenges WHERE id=$1 AND expires_at>clock_timestamp()",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .context("challenge is absent, consumed or expired")?;
        ensure!(
            row.network == self.network(row.chain.parse()?),
            "challenge network changed"
        );
        ensure!(
            row.message
                .contains(&format!("\nURI: {}\n", self.config.origin)),
            "challenge origin changed"
        );
        Ok(row)
    }

    async fn verify_challenge(
        &self,
        challenge: &ChallengeRow,
        signature: &str,
        public_key: Option<&str>,
    ) -> Result<SessionProof> {
        ensure!(signature.len() <= 16_384, "wallet signature is too large");
        match challenge.chain.parse()? {
            Chain::Ethereum => {
                let address: Address = challenge.address.parse()?;
                if signature
                    .parse::<Signature>()
                    .ok()
                    .and_then(|sig| sig.recover_address_from_msg(&challenge.message).ok())
                    == Some(address)
                {
                    return Ok(SessionProof::Key);
                }
                self.verify_contract_signature(&challenge.address, &challenge.message, signature)
                    .await?;
                Ok(SessionProof::EthereumContract {
                    message: challenge.message.clone(),
                    signature: signature.to_owned(),
                })
            }
            Chain::Solana => {
                let key = bs58::decode(&challenge.address).into_vec()?;
                let key = VerifyingKey::from_bytes(
                    &key.try_into()
                        .map_err(|_| anyhow::anyhow!("invalid Solana address"))?,
                )?;
                let signature = Ed25519Signature::from_slice(&bs58::decode(signature).into_vec()?)?;
                key.verify_strict(challenge.message.as_bytes(), &signature)
                    .context("invalid Solana login signature")?;
                Ok(SessionProof::Key)
            }
            Chain::Near => {
                let public_key = public_key.context("NEP-413 requires the signing public key")?;
                let key: near_crypto::PublicKey = public_key
                    .parse()
                    .context("invalid NEAR login public key")?;
                let signature = near_crypto::Signature::from_parts(
                    key.key_type(),
                    &STANDARD.decode(signature)?,
                )?;
                let nonce: [u8; 32] = challenge
                    .nonce
                    .as_slice()
                    .try_into()
                    .context("invalid challenge nonce")?;
                let digest = nep413_digest(
                    &challenge.message,
                    nonce,
                    &self.domain,
                    challenge.callback_url.as_deref(),
                )?;
                ensure!(signature.verify(&digest, &key), "invalid NEP-413 signature");
                self.verify_near_access(&challenge.address, public_key)
                    .await?;
                Ok(SessionProof::Near {
                    public_key: public_key.to_owned(),
                })
            }
        }
    }

    async fn verify_near_access(&self, account: &str, public_key: &str) -> Result<()> {
        ensure!(
            self.near_key_is_current(account, public_key).await?,
            "NEAR login key lacks full access"
        );
        Ok(())
    }

    async fn near_key_is_current(&self, account: &str, public_key: &str) -> Result<bool> {
        let block = self.near.finalized_block().await?;
        self.near
            .has_full_access_key(account, public_key, &block)
            .await
    }

    async fn verify_contract_signature(
        &self,
        address: &str,
        message: &str,
        signature: &str,
    ) -> Result<()> {
        let chain_id: String = self.ethereum.call("eth_chainId", &json!([])).await?;
        ensure!(
            u64::from_str_radix(chain_id.trim_start_matches("0x"), 16)?
                == self.config.ethereum_chain_id,
            "Ethereum authentication chain mismatch"
        );
        let genesis: Value = self
            .ethereum
            .call("eth_getBlockByNumber", &json!(["0x0", false]))
            .await?;
        ensure!(
            genesis["hash"] == self.config.ethereum_genesis_hash,
            "Ethereum authentication genesis mismatch"
        );
        let block: Value = self
            .ethereum
            .call("eth_getBlockByNumber", &json!(["finalized", false]))
            .await?;
        let hash = block["hash"]
            .as_str()
            .context("finalized Ethereum block unavailable")?;
        let at = json!({"blockHash": hash, "requireCanonical": true});
        let code: String = self
            .ethereum
            .call("eth_getCode", &json!([address, at]))
            .await?;
        ensure!(code != "0x", "invalid Ethereum login signature");
        let signature = hex::decode(
            signature
                .strip_prefix("0x")
                .context("Ethereum signature must be hex-prefixed")?,
        )?;
        let call = isValidSignatureCall {
            hash: eip191_hash_message(message),
            signature: signature.into(),
        };
        let result: String = self
            .ethereum
            .call(
                "eth_call",
                &json!([{
            "to": address, "data": format!("0x{}", hex::encode(call.abi_encode())),
        }, at]),
            )
            .await?;
        let bytes = hex::decode(
            result
                .strip_prefix("0x")
                .context("invalid ERC-1271 response")?,
        )?;
        let magic = isValidSignatureCall::abi_decode_returns_validate(&bytes)?;
        ensure!(
            magic.as_slice() == [0x16, 0x26, 0xba, 0x7e],
            "ERC-1271 signature rejected"
        );
        Ok(())
    }

    /// Canonical authentication identity used by verified wallet records.
    /// Worker transaction networks are separate identifiers and must not be
    /// used to compare profile wallets.
    pub(crate) fn network(&self, chain: Chain) -> String {
        match chain {
            Chain::Ethereum => format!(
                "{}:{}",
                self.config.ethereum_chain_id, self.config.ethereum_genesis_hash
            ),
            Chain::Solana => self.config.solana_genesis_hash.clone(),
            Chain::Near => self.config.near_network.clone(),
        }
    }

    fn sign_in_message(
        &self,
        chain: Chain,
        address: &str,
        nonce: &[u8; 32],
        statement: &str,
        now: DateTime<Utc>,
        id: Uuid,
    ) -> String {
        let expires = now + Duration::minutes(CHALLENGE_MINUTES);
        let (label, chain_id) = match chain {
            Chain::Ethereum => ("Ethereum", self.config.ethereum_chain_id.to_string()),
            Chain::Solana => ("Solana", self.config.solana_cluster.clone()),
            Chain::Near => ("NEAR", self.config.near_network.clone()),
        };
        format!(
            "{} wants you to sign in with your {label} account:\n{address}\n\n{statement}\n\nURI: {}\nVersion: 1\nChain ID: {chain_id}\nNonce: {}\nIssued At: {}\nExpiration Time: {}\nRequest ID: {id}\nResources:\n- urn:apodect:network:{}",
            self.domain,
            self.config.origin,
            hex::encode(nonce),
            now.to_rfc3339_opts(SecondsFormat::Millis, true),
            expires.to_rfc3339_opts(SecondsFormat::Millis, true),
            self.network(chain)
        )
    }
}

async fn consume_challenge(
    connection: &mut PgConnection,
    id: Uuid,
    session: Option<&Vec<u8>>,
) -> Result<()> {
    let consumed = sqlx::query("DELETE FROM login_challenges WHERE id=$1 AND expires_at>clock_timestamp() AND link_session IS NOT DISTINCT FROM $2")
        .bind(id).bind(session).execute(connection).await?.rows_affected();
    ensure!(consumed == 1, "challenge is absent, consumed or expired");
    Ok(())
}

async fn lock_session(connection: &mut PgConnection, digest: &[u8]) -> Result<()> {
    let active: Option<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM sessions WHERE token_hash=$1 AND expires_at>clock_timestamp() FOR UPDATE")
        .bind(digest).fetch_optional(connection).await?;
    ensure!(active.is_some(), "session is absent or expired");
    Ok(())
}

async fn lock_wallet(connection: &mut PgConnection, challenge: &ChallengeRow) -> Result<()> {
    let key = format!(
        "{}:{}:{}",
        challenge.chain, challenge.network, challenge.address
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(connection)
        .await?;
    Ok(())
}

async fn save_wallet(
    connection: &mut PgConnection,
    challenge: &ChallengeRow,
    profile: Uuid,
    proof: &SessionProof,
) -> Result<()> {
    let public_key = match proof {
        SessionProof::Near { public_key } => Some(public_key.as_str()),
        _ => None,
    };
    // The caller has verified this fresh proof and holds the wallet identity
    // lock. An existing link may refresh its key but never move to a profile.
    let saved = sqlx::query(
        "INSERT INTO wallets(chain,network,address,profile_id,near_public_key) VALUES($1,$2,$3,$4,$5) \
         ON CONFLICT(chain,network,address) DO UPDATE SET near_public_key=excluded.near_public_key \
         WHERE wallets.profile_id=excluded.profile_id",
    )
    .bind(&challenge.chain)
    .bind(&challenge.network)
    .bind(&challenge.address)
    .bind(profile)
    .bind(public_key)
    .execute(connection)
    .await?;
    ensure!(
        saved.rows_affected() == 1,
        "wallet belongs to another profile"
    );
    Ok(())
}

async fn read_profile(connection: &mut PgConnection, id: Uuid) -> Result<Profile> {
    Ok(
        sqlx::query_as("SELECT id,display_name,username FROM profiles WHERE id=$1")
            .bind(id)
            .fetch_one(connection)
            .await?,
    )
}

fn normalize_username(value: &str) -> Result<String> {
    let value = value.trim();
    let value = value.strip_prefix('@').unwrap_or(value);
    ensure!(
        (3..=32).contains(&value.len())
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        ProfileError::InvalidUsername
    );
    Ok(value.to_ascii_lowercase())
}

fn normalize_address(chain: Chain, address: &str) -> Result<String> {
    match chain {
        Chain::Ethereum => Ok(address.parse::<Address>()?.to_checksum(None)),
        Chain::Solana => {
            let bytes = bs58::decode(address).into_vec()?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid Solana address"))?;
            VerifyingKey::from_bytes(&bytes)?;
            Ok(bs58::encode(bytes).into_string())
        }
        Chain::Near => Ok(address
            .parse::<near_primitives::types::AccountId>()?
            .to_string()),
    }
}

fn random_bytes() -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .context("secure randomness unavailable")?;
    Ok(bytes)
}

fn token_hash(token: &str) -> Result<Vec<u8>> {
    ensure!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid session token"
    );
    Ok(Sha256::digest(hex::decode(token)?).to_vec())
}

fn nep413_digest(
    message: &str,
    nonce: [u8; 32],
    recipient: &str,
    callback_url: Option<&str>,
) -> Result<[u8; 32]> {
    #[derive(borsh::BorshSerialize)]
    struct Payload<'a> {
        tag: u32,
        message: &'a str,
        nonce: [u8; 32],
        recipient: &'a str,
        callback_url: Option<&'a str>,
    }
    let payload = Payload {
        tag: (1 << 31) + 413,
        message,
        nonce,
        recipient,
        callback_url,
    };
    Ok(Sha256::digest(borsh::to_vec(&payload)?).into())
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
