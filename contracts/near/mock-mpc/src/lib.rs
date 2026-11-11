//! Explicit local simulator. The responder replaces MPC quorum trust, not coordinator authorization.

use apodect_near_protocol::{
    SignRequest, SignatureResponse, VerificationResponse, VerifyForeignTransactionRequest, Word,
    hex, unhex, word,
};
use near_sdk::store::LookupMap;
use near_sdk::{
    AccountId, Gas, GasWeight, PanicOnDefault, env, near, require,
    serde_json::{Value, json},
};

#[near(serializers = [borsh])]
struct Pending {
    yield_id: Word,
    kind: u8,
    digest: Word,
    request: String,
    answered: bool,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct MockMpc {
    coordinator_id: AccountId,
    responder_id: AccountId,
    sign_key: Vec<u8>,
    read_key: Vec<u8>,
    sign_path: String,
    next_request: u64,
    pending: LookupMap<u64, Pending>,
}

#[near]
impl MockMpc {
    #[init]
    pub fn new(
        coordinator_id: AccountId,
        responder_id: AccountId,
        sign_key: String,
        read_key: String,
        sign_path: String,
    ) -> Self {
        let sign_key = unhex(&sign_key);
        let read_key = unhex(&read_key);
        require!(
            sign_key.len() == 64 && read_key.len() == 64,
            "INVALID_PUBLIC_KEY"
        );
        Self {
            coordinator_id,
            responder_id,
            sign_key,
            read_key,
            sign_path,
            next_request: 0,
            pending: LookupMap::new(b"p"),
        }
    }

    #[payable]
    pub fn sign(&mut self, request: SignRequest) {
        require!(
            request.domain_id == 0 && request.path == self.sign_path,
            "WRONG_SIGNING_DOMAIN"
        );
        self.enqueue(
            0,
            request.digest(),
            near_sdk::serde_json::to_string(&request).unwrap(),
        );
    }

    #[payable]
    pub fn verify_foreign_transaction(&mut self, request: VerifyForeignTransactionRequest) {
        require!(
            request.domain_id == 3 && request.payload_version == 1,
            "WRONG_VERIFICATION_DOMAIN"
        );
        let digest = word(&request.expected_payload_hash);
        self.enqueue(
            1,
            digest,
            near_sdk::serde_json::to_string(&request).unwrap(),
        );
    }

    pub fn respond(&mut self, request_id: u64, response: Value) -> bool {
        require!(
            env::predecessor_account_id() == self.responder_id,
            "ONLY_LOCAL_RESPONDER"
        );
        let pending = self
            .pending
            .get_mut(&request_id)
            .expect("REQUEST_NOT_FOUND");
        if pending.answered {
            return false;
        }
        if pending.kind == 0 {
            let sig: SignatureResponse =
                near_sdk::serde_json::from_value(response.clone()).expect("INVALID_SIGN_RESPONSE");
            sig.verify(pending.digest, &self.sign_key);
        } else {
            let result: VerificationResponse =
                near_sdk::serde_json::from_value(response.clone()).expect("INVALID_READ_RESPONSE");
            require!(
                word(&result.payload_hash) == pending.digest,
                "READ_HASH_MISMATCH"
            );
            result.signature.verify(pending.digest, &self.read_key);
        }
        let ok = env::promise_yield_resume(
            &pending.yield_id,
            &near_sdk::serde_json::to_vec(&response).unwrap(),
        );
        pending.answered = ok;
        ok
    }

    #[private]
    pub fn finish(
        &mut self,
        request_id: u64,
        #[callback_result] result: Result<Value, near_sdk::PromiseError>,
    ) -> Value {
        self.pending
            .get_mut(&request_id)
            .expect("REQUEST_NOT_FOUND")
            .answered = true;
        // Panicking here would roll back the terminal marker and leave an
        // expired yield request looking live forever. Null is an invalid MPC
        // response: the coordinator rejects it without authorizing a command.
        result.unwrap_or(Value::Null)
    }

    pub fn get_state(&self) -> Value {
        json!({
            "next_request": self.next_request,
            "coordinator_id": self.coordinator_id,
            "responder_id": self.responder_id,
            "local_simulator": true
        })
    }

    pub fn get_pending(&self, request_id: u64) -> Value {
        self.pending
            .get(&request_id)
            .map(|p| {
                json!({
                    "request_id": request_id,
                    "kind": if p.kind == 0 { "sign" } else { "verify" },
                    "digest": hex(&p.digest),
                    "request": near_sdk::serde_json::from_str::<Value>(&p.request).unwrap(),
                    "answered": p.answered
                })
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::{test_utils::VMContextBuilder, testing_env};

    #[test]
    fn timed_out_yield_closes_request_without_a_signing_response() {
        testing_env!(
            VMContextBuilder::new()
                .current_account_id("mpc.test.near".parse().unwrap())
                .predecessor_account_id("mpc.test.near".parse().unwrap())
                .build()
        );
        let mut mock = MockMpc::new(
            "coordinator.test.near".parse().unwrap(),
            "provider.test.near".parse().unwrap(),
            "11".repeat(64),
            "22".repeat(64),
            "test".into(),
        );
        mock.pending.insert(
            0,
            Pending {
                yield_id: [1; 32],
                kind: 0,
                digest: [2; 32],
                request: "{}".into(),
                answered: false,
            },
        );
        assert_eq!(
            mock.finish(0, Err(near_sdk::PromiseError::Failed)),
            Value::Null
        );
        mock.pending.flush();
        assert_eq!(mock.get_pending(0)["answered"], true);
    }
}

impl MockMpc {
    fn enqueue(&mut self, kind: u8, digest: Word, request: String) {
        require!(
            env::predecessor_account_id() == self.coordinator_id,
            "ONLY_COORDINATOR"
        );
        require!(
            env::attached_deposit().as_yoctonear() > 0,
            "DEPOSIT_REQUIRED"
        );
        let request_id = self.next_request;
        self.next_request = self.next_request.checked_add(1).expect("REQUEST_OVERFLOW");
        let promise = env::promise_yield_create(
            "finish",
            json!({"request_id": request_id}).to_string().as_bytes(),
            Gas::from_tgas(12),
            GasWeight(0),
            0,
        );
        let yield_id = env::read_register(0)
            .expect("MISSING_YIELD_ID")
            .try_into()
            .expect("INVALID_YIELD_ID");
        self.pending.insert(
            request_id,
            Pending {
                yield_id,
                kind,
                digest,
                request,
                answered: false,
            },
        );
        env::promise_return(promise);
    }
}
