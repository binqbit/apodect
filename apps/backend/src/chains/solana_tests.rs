use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
};

use anchor_lang::AccountSerialize;
use apodect_solana::state::{RequestKind, RequestState, TaskLifecycle};

use super::*;

fn mock_rpc(responses: Vec<(&'static str, Value)>) -> (Rpc, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let handle = thread::spawn(move || {
        for (method, result) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut length = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["method"], method);
            if matches!(method, "getAccountInfo" | "getMultipleAccounts") {
                assert_eq!(request["params"][1]["commitment"], "finalized");
            }
            if method == "getBlockHeight" {
                assert_eq!(request["params"][0]["commitment"], "finalized");
            }
            let response =
                json!({"jsonrpc":"2.0", "id":request["id"], "result":result}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", response.len(), response).unwrap();
        }
    });
    (rpc, handle)
}

fn account(value: &impl AccountSerialize) -> Value {
    let mut bytes = vec![];
    value.try_serialize(&mut bytes).unwrap();
    json!({"owner":apodect_solana::ID.to_string(),"executable":false,"data":[STANDARD.encode(bytes),"base64"]})
}

#[tokio::test]
async fn scan_keeps_zero_indices_and_rejects_foreign_request() {
    for foreign in [false, true] {
        let workspace = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let (project, bump) = Pubkey::find_program_address(
            &[b"project", workspace.as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        );
        let task = bridge::pda(&[b"task", project.as_ref(), &0u64.to_le_bytes()]);
        let request = bridge::pda(&[b"request", task.as_ref(), &0u64.to_le_bytes()]);
        let state = Project {
            workspace,
            index: 0,
            bump,
            authority: Pubkey::new_unique(),
            mint,
            free: 10,
            task_count: 1,
            invoice_count: 0,
            approval_count: 0,
            head: None,
            tail: None,
            domain: [1; 32],
            command_key: [2; 64],
            held: 0,
            checkpoint_sequence: 0,
        };
        let task_state = Task {
            project,
            agent: Pubkey::new_unique(),
            lifecycle: TaskLifecycle::Active,
            paused: false,
            started: false,
            available: 0,
            issued: 0,
            recovered: 0,
            request_count: 1,
            pending: Some(request),
            report: ZERO,
            control_nonce: 0,
        };
        let request_state = BudgetRequest {
            project: if foreign {
                Pubkey::new_unique()
            } else {
                project
            },
            task,
            amount: 10,
            purpose: [3; 32],
            kind: RequestKind::Initial,
            state: RequestState::Pending,
            approval_sequence: 0,
            next: None,
        };
        let (rpc, server) = mock_rpc(vec![
            (
                "getAccountInfo",
                json!({"context":{"slot":10},"value":account(&state)}),
            ),
            (
                "getMultipleAccounts",
                json!({"context":{"slot":11},"value":[account(&task_state)]}),
            ),
            (
                "getMultipleAccounts",
                json!({"context":{"slot":12},"value":[account(&request_state)]}),
            ),
        ]);
        let client = Client::new(
            rpc,
            Binding {
                genesis_hash: "genesis".into(),
                project,
                mint,
                domain: [1; 32],
            },
            None,
        )
        .unwrap();
        let result = client.scan_tasks(0, 10).await;
        server.join().unwrap();

        if foreign {
            assert!(result.is_err());
        } else {
            let page = result.unwrap();
            assert_eq!(page.next_task, 1);
            assert_eq!(page.tasks.len(), 1);
            assert_eq!(page.tasks[0].address, task);
            assert_eq!(page.tasks[0].request.as_ref().unwrap().address, request);
        }
    }
}

#[tokio::test]
async fn unfinalized_failures_remain_pending_and_missing_signatures_remain_unknown() {
    let signature = Signature::from([7; 64]).to_string();
    let (rpc, server) = mock_rpc(vec![
        (
            "getSignatureStatuses",
            json!({"context":{"slot":10},"value":[null]}),
        ),
        (
            "getSignatureStatuses",
            json!({"context":{"slot":10},"value":[{"slot":9,"err":{"InstructionError":[0,"Custom"]},"confirmationStatus":"confirmed"}]}),
        ),
        (
            "getSignatureStatuses",
            json!({"context":{"slot":11},"value":[{"slot":9,"err":{"InstructionError":[0,"Custom"]},"confirmationStatus":"finalized"}]}),
        ),
        (
            "getSignatureStatuses",
            json!({"context":{"slot":11},"value":[{"slot":9,"err":null,"confirmationStatus":"finalized"}]}),
        ),
        ("getBlockHeight", json!(200)),
        ("getBlockHeight", json!(201)),
    ]);
    let client = Client::new(
        rpc,
        Binding {
            genesis_hash: "genesis".into(),
            project: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            domain: [1; 32],
        },
        None,
    )
    .unwrap();
    assert_eq!(
        client.transaction_outcome(&signature).await.unwrap(),
        TransactionOutcome::Unknown
    );
    assert_eq!(
        client.transaction_outcome(&signature).await.unwrap(),
        TransactionOutcome::Pending
    );
    assert!(matches!(
        client.transaction_outcome(&signature).await.unwrap(),
        TransactionOutcome::Failed { slot: 9, .. }
    ));
    assert_eq!(
        client.transaction_outcome(&signature).await.unwrap(),
        TransactionOutcome::Finalized { slot: 9 }
    );
    let transaction = PreparedTransaction {
        signature,
        bytes_base64: String::new(),
        last_valid_block_height: 200,
    };
    assert!(!client.transaction_expired(&transaction).await.unwrap());
    assert!(client.transaction_expired(&transaction).await.unwrap());
    server.join().unwrap();
}

#[test]
fn immutable_evidence_rejects_forged_provenance_and_future_state() {
    let domain = [1; 32];
    let id = [2; 32];
    let payer = Pubkey::new_unique();
    let mut funding = vec![0; 168];
    funding[..8].copy_from_slice(FundingReceipt::DISCRIMINATOR);
    funding[8..40].copy_from_slice(&domain);
    funding[40..72].copy_from_slice(&id);
    funding[104..136].copy_from_slice(&bridge::word(10));
    funding[136..168].copy_from_slice(payer.as_ref());
    let mut account = RpcAccount {
        owner: apodect_solana::ID.to_string(),
        executable: false,
        data: (String::new(), "base64".into()),
    };
    assert!(validate_receipt(&account, &funding, domain, id, true, Some(payer), 20).is_ok());

    assert!(validate_receipt(&account, &funding, [3; 32], id, true, Some(payer), 20).is_err());
    assert!(validate_receipt(&account, &funding, domain, [3; 32], true, Some(payer), 20).is_err());
    assert!(
        validate_receipt(
            &account,
            &funding,
            domain,
            id,
            true,
            Some(Pubkey::new_unique()),
            20
        )
        .is_err()
    );
    account.owner = Pubkey::new_unique().to_string();
    assert!(validate_receipt(&account, &funding, domain, id, true, Some(payer), 20).is_err());
    account.owner = apodect_solana::ID.to_string();
    account.executable = true;
    assert!(validate_receipt(&account, &funding, domain, id, true, Some(payer), 20).is_err());
    account.executable = false;

    let mut checkpoint = vec![0; 1256];
    checkpoint[..8].copy_from_slice(Checkpoint::DISCRIMINATOR);
    checkpoint[8..40].copy_from_slice(&domain);
    checkpoint[40..72].copy_from_slice(&id);
    checkpoint[104..136].copy_from_slice(&bridge::word(21));
    assert!(validate_receipt(&account, &checkpoint, domain, id, false, None, 20).is_err());
    assert!(validate_receipt(&account, &checkpoint, domain, id, false, None, 21).is_ok());
    checkpoint[0] ^= 1;
    assert!(validate_receipt(&account, &checkpoint, domain, id, false, None, 21).is_err());
}
