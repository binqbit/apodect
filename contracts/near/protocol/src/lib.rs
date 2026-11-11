//! The bounded cross-chain wire protocol and provider signature representation.

use near_sdk::{env, near, require};
pub mod foreign;

pub type Word = [u8; 32];
pub const ZERO: Word = [0; 32];
pub const COMMAND_WORDS: usize = 12;
pub const COMMAND_BYTES: usize = COMMAND_WORDS * 32;
// NEAR MPC 3.18.0 requires one yoctoNEAR for sign and foreign verification.
pub const MPC_REQUEST_DEPOSIT_YOCTONEAR: u128 = 1;

// Names for the fixed numeric wire values. These are constants, so persisted
// integers and the cross-chain payload encoding retain their existing layout.
pub mod action {
    pub const RESERVE: u64 = 1;
    pub const EXPECT_DELIVERY: u64 = 2;
    pub const EXPECT_REFUND: u64 = 3;
    pub const DISPATCH: u64 = 4;
    pub const ALLOCATE: u64 = 5;
    pub const CLOSE_PLAN: u64 = 6;
    pub const REJECT: u64 = 7;
    pub const OUT_OF_BUDGET: u64 = 8;
    pub const PAUSE: u64 = 9;
    pub const RECLAIM: u64 = 10;
    pub const CONTINUE: u64 = 11;
    pub const CANCEL: u64 = 12;
    pub const RESTORE_REQUEST: u64 = 13;
    pub const OWNER_WITHDRAW: u64 = 14;
    pub const INVALIDATE: u64 = 15;
}

pub mod command_state {
    pub const UNOBSERVED: u64 = 0;
    pub const SUCCESS: u64 = 1;
    pub const FAILED: u64 = 2;
    pub const INVALIDATED: u64 = 3;
    pub const PENDING: u64 = 4;

    pub fn is_terminal(state: u64) -> bool {
        matches!(state, SUCCESS | FAILED | INVALIDATED)
    }

    pub fn is_unresolved(state: u64) -> bool {
        matches!(state, UNOBSERVED | PENDING)
    }
}

pub mod task_state {
    pub const ACTIVE: u64 = 0;
    pub const STOPPED: u64 = 1;
    pub const OUT_OF_BUDGET: u64 = 2;
    pub const COMPLETED: u64 = 3;
    pub const CANCELED: u64 = 4;
    pub const MISSING: u64 = 255;
}

pub mod request_kind {
    pub const INITIAL: u64 = 0;
    pub const SUPPLEMENT: u64 = 1;
    pub const RESUME: u64 = 2;
    pub const RESTORATION: u64 = 3;
}

pub mod request_state {
    pub const PENDING: u64 = 0;
    pub const QUEUED: u64 = 1;
    pub const FUNDED: u64 = 2;
    pub const REJECTED: u64 = 3;
    pub const REMOVED: u64 = 4;
    pub const MISSING: u64 = 255;
}

pub mod plan_state {
    pub const MISSING: u64 = 0;
    pub const OPEN: u64 = 1;
    pub const ALLOCATED: u64 = 2;
    pub const CLOSED: u64 = 3;
}

pub mod route_kind {
    pub const MISSING: u64 = 0;
    pub const DELIVERY: u64 = 1;
    pub const REFUND: u64 = 2;
}

const HALF_ORDER: Word = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 15)] as char);
    }
    output
}

pub fn unhex(value: &str) -> Vec<u8> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    require!(value.len().is_multiple_of(2), "INVALID_HEX_LENGTH");
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let nibble = |byte: u8| match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => env::panic_str("INVALID_HEX"),
            };
            nibble(pair[0]) * 16 + nibble(pair[1])
        })
        .collect()
}

pub fn word(value: &str) -> Word {
    let value = value.strip_prefix("0x").unwrap_or(value);
    require!(value.len() == 64, "INVALID_WORD_LENGTH");
    unhex(value).try_into().expect("INVALID_WORD_LENGTH")
}

pub fn uint(value: u64) -> Word {
    let mut result = ZERO;
    result[24..].copy_from_slice(&value.to_be_bytes());
    result
}

pub fn number(value: Word) -> u64 {
    require!(value[..24] == [0; 24], "WORD_EXCEEDS_U64");
    u64::from_be_bytes(value[24..].try_into().unwrap())
}

pub fn hash(bytes: &[u8]) -> Word {
    env::sha256_array(bytes)
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct Command {
    pub words: [Word; COMMAND_WORDS],
}

impl Command {
    pub fn parse(value: &str) -> Self {
        let value = value.strip_prefix("0x").unwrap_or(value);
        require!(value.len() == COMMAND_BYTES * 2, "INVALID_COMMAND_LENGTH");
        let bytes = unhex(value);
        let mut words = [ZERO; COMMAND_WORDS];
        for (word, bytes) in words.iter_mut().zip(bytes.chunks_exact(32)) {
            word.copy_from_slice(bytes);
        }
        Self { words }
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.words.iter().flatten().copied().collect()
    }

    pub fn digest(&self) -> Word {
        hash(&self.bytes())
    }

    pub fn hex(&self) -> String {
        hex(&self.bytes())
    }

    pub fn action(&self) -> u64 {
        number(self.words[2])
    }

    pub fn operation(&self) -> Word {
        self.words[3]
    }

    pub fn plan(&self) -> Word {
        self.words[4]
    }

    pub fn amount(&self) -> u64 {
        number(self.words[7])
    }

    pub fn validate(&self, domain: Word) {
        require!(self.words[0] == domain, "WRONG_COMMAND_DOMAIN");
        require!(self.operation() != ZERO, "ZERO_OPERATION");
        require!(
            (action::RESERVE..=action::INVALIDATE).contains(&self.action()),
            "UNKNOWN_ACTION"
        );
        require!(number(self.words[8]) == 0, "NONZERO_RESERVED_LIMIT");
        require!(
            env::block_timestamp_ms() / 1000 <= number(self.words[10]),
            "COMMAND_EXPIRED"
        );
        self.amount();
        let control = matches!(self.action(), action::PAUSE..=action::CANCEL);
        if !control {
            require!(self.words[1] == ZERO, "NONZERO_UNUSED_NONCE");
        }
        if !matches!(self.action(), action::DISPATCH | action::OWNER_WITHDRAW) {
            require!(self.words[9] == ZERO, "NONZERO_UNUSED_RECIPIENT");
        }
        if matches!(self.action(), action::REJECT..=action::INVALIDATE) {
            require!(self.plan() == ZERO, "NONZERO_UNUSED_PLAN");
        } else {
            require!(self.plan() != ZERO, "ZERO_PLAN");
        }
        if matches!(
            self.action(),
            action::CLOSE_PLAN
                | action::REJECT
                | action::OUT_OF_BUDGET
                | action::PAUSE
                | action::CONTINUE
                | action::CANCEL
                | action::INVALIDATE
        ) {
            require!(self.amount() == 0, "NONZERO_UNUSED_AMOUNT");
        }
        if matches!(
            self.action(),
            action::RESERVE
                | action::EXPECT_DELIVERY
                | action::EXPECT_REFUND
                | action::DISPATCH
                | action::CLOSE_PLAN
                | action::OWNER_WITHDRAW
                | action::INVALIDATE
        ) {
            require!(
                self.words[5] == ZERO && self.words[6] == ZERO,
                "NONZERO_UNUSED_TASK_REQUEST"
            );
        }
        if matches!(
            self.action(),
            action::OUT_OF_BUDGET..=action::RESTORE_REQUEST
        ) {
            require!(self.words[6] == ZERO, "NONZERO_UNUSED_REQUEST");
        }
        if matches!(
            self.action(),
            action::RESERVE
                | action::ALLOCATE
                | action::CLOSE_PLAN
                | action::REJECT
                | action::OUT_OF_BUDGET
                | action::PAUSE
                | action::RECLAIM
                | action::CONTINUE
                | action::CANCEL
                | action::OWNER_WITHDRAW
        ) {
            require!(self.words[11] == ZERO, "NONZERO_UNUSED_REFERENCE");
        } else {
            require!(self.words[11] != ZERO, "ZERO_REFERENCE");
        }
    }
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct AffinePoint {
    pub affine_point: String,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct Scalar {
    pub scalar: String,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct SignatureResponse {
    pub scheme: String,
    pub big_r: AffinePoint,
    pub s: Scalar,
    pub recovery_id: u8,
}

impl SignatureResponse {
    /// Provider responses are untrusted: callback validation must not roll back recovery state.
    pub fn is_valid(&self, digest: Word, key: &[u8]) -> bool {
        if self.scheme != "Secp256k1" || self.recovery_id > 1 || key.len() != 64 {
            return false;
        }
        let decode = |value: &str, length: usize| -> Option<Vec<u8>> {
            if value.len() != length * 2 {
                return None;
            }
            value
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let high = (pair[0] as char).to_digit(16)?;
                    let low = (pair[1] as char).to_digit(16)?;
                    Some((high * 16 + low) as u8)
                })
                .collect()
        };
        let Some(r) = decode(&self.big_r.affine_point, 33) else {
            return false;
        };
        let Some(s) = decode(&self.s.scalar, 32) else {
            return false;
        };
        if r[0] != 2 + self.recovery_id
            || s.iter().all(|byte| *byte == 0)
            || s.as_slice() > HALF_ORDER.as_slice()
        {
            return false;
        }
        let signature = [&r[1..], s.as_slice()].concat();
        env::ecrecover(&digest, &signature, self.recovery_id, true)
            .is_some_and(|recovered| recovered.as_slice() == key)
    }

    pub fn bytes(&self) -> [u8; 64] {
        require!(self.scheme == "Secp256k1", "WRONG_SIGNATURE_SCHEME");
        require!(self.big_r.affine_point.len() == 66, "INVALID_R_LENGTH");
        let r = unhex(&self.big_r.affine_point);
        require!(
            self.recovery_id <= 1 && r[0] == 2 + self.recovery_id,
            "INVALID_R_PREFIX"
        );
        let s = word(&self.s.scalar);
        let mut result = [0; 64];
        result[..32].copy_from_slice(&r[1..]);
        result[32..].copy_from_slice(&s);
        result
    }

    pub fn verify(&self, digest: Word, key: &[u8]) {
        verify_signature(digest, &self.bytes(), self.recovery_id, key);
    }
}

pub fn verify_signature(digest: Word, signature: &[u8], recovery_id: u8, key: &[u8]) {
    require!(
        key.len() == 64 && signature.len() == 64 && recovery_id <= 1,
        "INVALID_SIGNATURE_SHAPE"
    );
    let s: Word = signature[32..].try_into().unwrap();
    require!(s != ZERO && s <= HALF_ORDER, "NONCANONICAL_SIGNATURE");
    let recovered =
        env::ecrecover(&digest, signature, recovery_id, true).expect("INVALID_SIGNATURE");
    require!(
        recovered.as_slice() == key,
        "UNAUTHORIZED_COMMAND_SIGNATURE"
    );
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct SignRequest {
    pub path: String,
    pub domain_id: u64,
    pub payload_v2: SignPayload,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub enum SignPayload {
    Ecdsa(String),
}

impl SignRequest {
    pub fn digest(&self) -> Word {
        let SignPayload::Ecdsa(digest) = &self.payload_v2;
        word(digest)
    }
}

#[near(serializers = [json])]
pub struct VerifyForeignTransactionRequest {
    pub request: near_sdk::serde_json::Value,
    pub domain_id: u64,
    pub payload_version: u32,
    pub expected_payload_hash: String,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct VerificationResponse {
    pub payload_hash: String,
    pub signature: SignatureResponse,
}

pub const CHECKPOINT_WORDS: usize = 39;
pub const CHECKPOINT_BYTES: usize = CHECKPOINT_WORDS * 32;

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct Checkpoint {
    pub words: [Word; CHECKPOINT_WORDS],
}

impl Checkpoint {
    pub fn parse(value: &str) -> Self {
        let value = value.strip_prefix("0x").unwrap_or(value);
        require!(
            value.len() == CHECKPOINT_BYTES * 2,
            "INVALID_CHECKPOINT_LENGTH"
        );
        let bytes = unhex(value);
        let mut words = [ZERO; CHECKPOINT_WORDS];
        for (word, bytes) in words.iter_mut().zip(bytes.chunks_exact(32)) {
            word.copy_from_slice(bytes);
        }
        Self { words }
    }

    pub fn hex(&self) -> String {
        hex(&self.words.iter().flatten().copied().collect::<Vec<_>>())
    }

    pub fn value(&self, index: usize) -> u64 {
        number(self.words[index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_wire_hashes_match_independent_python_borsh_vectors() {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        let vectors: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(include_str!("../tests/fixtures/foreign-payloads.json"))
                .unwrap();
        for vector in vectors.as_array().unwrap() {
            let request = VerifyForeignTransactionRequest {
                request: vector["request"].clone(),
                domain_id: 3,
                payload_version: 1,
                expected_payload_hash: vector["hash"].as_str().unwrap().to_owned(),
            };
            let encoded = near_sdk::serde_json::to_value(&request).unwrap();
            assert_eq!(
                encoded,
                near_sdk::serde_json::json!({
                    "request": vector["request"],
                    "domain_id": 3,
                    "payload_version": 1,
                    "expected_payload_hash": vector["hash"]
                })
            );
            let decoded: VerifyForeignTransactionRequest =
                near_sdk::serde_json::from_value(encoded).unwrap();
            assert_eq!(
                hex(&foreign::foreign_hash(&decoded.request, &vector["values"])),
                decoded.expected_payload_hash
            );
        }
    }

    #[test]
    #[should_panic(expected = "FINALIZED_REQUIRED")]
    fn optimistic_foreign_evidence_is_rejected() {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        let mut v: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(include_str!("../tests/fixtures/foreign-payloads.json"))
                .unwrap();
        v[0]["request"]["Ethereum"]["finality"] = "Latest".into();
        foreign::foreign_hash(&v[0]["request"], &v[0]["values"]);
    }
}
