pub mod accounts;
pub mod actions;
pub mod api;
pub mod application;
pub mod chains;
pub mod config;
pub mod database;
pub mod deployment;
pub mod events;
pub mod indexing;
pub mod liquidity;
pub mod operations;
pub mod providers;
pub mod relay;
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

/// Optional identifiers follow the same decimal-string wire format as amounts.
pub(crate) mod optional_decimal_u64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        value.map(|value| value.to_string()).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(D::Error::custom(
                        "expected an unsigned decimal identifier string",
                    ));
                }
                value.parse().map_err(D::Error::custom)
            })
            .transpose()
    }
}
