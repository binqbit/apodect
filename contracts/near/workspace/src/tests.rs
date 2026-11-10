//! Factory behavior and fixed serialization/API compatibility checks.

use crate::{Creation, CreationState, Workspace};
use near_sdk::borsh::{self, BorshSerialize};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{AccountId, NearToken, env, testing_env};
use std::collections::BTreeMap;

#[test]
fn initialization_binds_owner_and_asset() {
    let account: AccountId = "workspace.test.near".parse().unwrap();
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(account.clone())
            .predecessor_account_id(account)
            .build()
    );
    let workspace = Workspace::new(
        "owner.test.near".parse().unwrap(),
        "usdc.test.near".parse().unwrap(),
    );
    assert_eq!(workspace.get_workspace().next_project, 0);
    assert_eq!(
        workspace.get_workspace().owner_id.as_str(),
        "owner.test.near"
    );
}

fn encoded(value: &impl BorshSerialize) -> String {
    hex(&borsh::to_vec(value).unwrap())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn state_compatibility() {
    near_sdk::mock::with_mocked_blockchain(|blockchain| {
        blockchain.take_storage();
    });
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("workspace.test.near".parse().unwrap())
            .predecessor_account_id("workspace.test.near".parse().unwrap())
            .account_balance(NearToken::from_near(100))
            .build()
    );
    let mut workspace = Workspace::new(
        "owner.test.near".parse().unwrap(),
        "token.test.near".parse().unwrap(),
    );
    workspace.next_project = 17;
    let creation = Creation {
        account_id: "p16.workspace.test.near".parse().unwrap(),
        state: CreationState::Failed,
        initial_balance: 91,
        refund_credit: 81,
        refund_pending: true,
    };
    let record = encoded(&creation);
    workspace.projects.insert(16, creation);
    workspace.projects.flush();
    env::state_write(&workspace);
    let json_api = near_sdk::serde_json::json!({"workspace":workspace.get_workspace(),"creation":workspace.get_project(16)});
    let storage: BTreeMap<_, _> =
        near_sdk::mock::with_mocked_blockchain(|blockchain| blockchain.take_storage())
            .into_iter()
            .map(|(key, value)| (hex(&key), hex(&value)))
            .collect();
    let actual = near_sdk::serde_json::json!({"root":encoded(&workspace),"creation":record,"storage":storage,"json_api":json_api,"creation_states":[encoded(&CreationState::Pending),encoded(&CreationState::Ready),encoded(&CreationState::Failed)]});
    let expected: near_sdk::serde_json::Value =
        near_sdk::serde_json::from_str(include_str!("tests/fixtures/state-v1.json")).unwrap();
    assert_eq!(actual, expected);
}
