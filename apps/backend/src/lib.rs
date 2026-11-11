pub mod accounts;
pub mod actions;
pub mod chains;
pub mod config;
pub mod database;
pub mod indexing;
pub mod liquidity;
pub mod rpc;
pub mod storage;

/// Wallet-facing monetary values use decimal strings so JavaScript cannot round them.
pub(crate) mod decimal_u64 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(amount: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&amount.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(D::Error::custom(
                "expected an unsigned decimal amount string",
            ));
        }
        value.parse().map_err(D::Error::custom)
    }
}
