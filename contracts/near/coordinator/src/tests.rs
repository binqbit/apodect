use super::*;
use apodect_near_protocol::Checkpoint;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;

fn context(role: &str) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("coordinator.test.near".parse().unwrap())
            .predecessor_account_id(format!("{role}.test.near").parse().unwrap())
            .attached_deposit(NearToken::from_near(10))
            .account_balance(NearToken::from_near(100))
            .prepaid_gas(Gas::from_tgas(300))
            .build()
    );
}
fn fixture() -> Coordinator {
    context("owner");
    let mut c = Coordinator::new(
        "owner.test.near".parse().unwrap(),
        "authority.test.near".parse().unwrap(),
        hex(&uint(50)),
        "mpc.test.near".parse().unwrap(),
        "11".repeat(64),
        "22".repeat(64),
        "test".into(),
    );
    c.register_domain(
        hex(&uint(1)),
        "near".into(),
        "project.test.near".into(),
        "token.test.near".into(),
        hex(&hash(b"owner.test.near")),
        hex(&hash(b"owner.test.near")),
        None,
    );
    c.register_domain(
        hex(&uint(2)),
        "ethereum".into(),
        "11".repeat(20),
        "22".repeat(20),
        hex(&uint(42)),
        hex(&uint(42)),
        None,
    );
    c.consume(snapshot(1, 1, 100));
    c.consume(snapshot(2, 1, 100));
    c
}
fn snapshot(domain: u64, sequence: u64, free: u64) -> Checkpoint {
    let mut p = Checkpoint { words: [ZERO; 39] };
    p.words[0] = uint(domain);
    p.words[1] = uint(sequence);
    p.words[2] = uint(sequence);
    p.words[5] = uint(free);
    p.words[8] = uint(255);
    p.words[21] = uint(255);
    p
}
fn request(c: &mut Coordinator, id: u64, amount: u64) {
    let mut p = snapshot(1, id + 2, 100);
    p.words[7] = uint(id);
    p.words[8] = ZERO;
    p.words[17] = uint(id);
    p.words[18] = uint(id);
    p.words[19] = uint(amount);
    p.words[20] = ZERO;
    p.words[21] = ZERO;
    p.words[22] = uint(1);
    p.words[37] = uint(id);
    p.words[38] = uint(1);
    c.consume(p);
}
fn ack(c: &mut Coordinator, op: u64, seq: u64, free: u64) {
    let r = c.commands.get(&uint(op)).unwrap();
    let mut p = snapshot(number(r.command.words[0]), seq, free);
    p.words[32] = uint(op);
    p.words[33] = r.command.digest();
    p.words[34] = r.command.words[2];
    p.words[35] = uint(1);
    p.words[36] = r.command.words[7];
    if r.command.action() == 6 {
        p.words[23] = r.command.plan();
        let plan = c.plans.get(&r.command.plan()).unwrap();
        p.words[24] = uint(
            if plan.destination == r.command.words[0]
                && plan.allocation.is_some_and(|op| c.succeeded(op))
            {
                2
            } else {
                3
            },
        );
    }
    c.consume(p);
}

#[test]
fn local_full_plan_and_common_fifo() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    request(&mut c, 1, 20);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.approve(hex(&uint(1)), hex(&uint(1)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    assert_eq!(c.head, 0);
    assert_eq!(c.available(uint(1)), 40);
    ack(&mut c, 1, 5, 40);
    assert_eq!(c.domain(uint(1)).pending_reserves, 0);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.head, 1);
    assert_eq!(c.get_plan(plan.clone())["status"], "Allocating");
    ack(&mut c, 2, 6, 40);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan.clone())["status"], "Settling");
    ack(&mut c, 3, 7, 40);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan)["status"], "Funded");
}

#[test]
fn authority_resume_replaces_only_proven_terminal_closures() {
    for allocated in [false, true] {
        for terminal in [2, 3] {
            let mut c = fixture();
            request(&mut c, 0, 60);
            context("authority");
            c.approve(hex(&uint(1)), hex(&uint(0)));
            let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
            ack(&mut c, 1, 4, 40);
            if allocated {
                context("relayer");
                c.advance(plan.clone());
                ack(&mut c, 2, 5, 40);
                context("relayer");
                c.advance(plan.clone());
            } else {
                context("authority");
                c.abort(plan.clone());
            }
            let closure = c.plans.get(&word(&plan)).unwrap().closures[0];
            let original = c.commands.get(&closure).unwrap().command.hex();
            let count = c.next_command;
            // A deadline or attention reset does not prove this closure failed.
            context("authority");
            c.resume_plan(plan.clone());
            assert_eq!(c.next_command, count);
            assert_eq!(c.plans.get(&word(&plan)).unwrap().closures, [closure]);

            let mut proof = snapshot(1, 6, 40);
            proof.words[32] = closure;
            proof.words[33] = c.commands.get(&closure).unwrap().command.digest();
            proof.words[34] = uint(6);
            proof.words[35] = uint(terminal);
            c.consume(proof);
            context("relayer");
            c.advance(plan.clone());
            assert_eq!(c.next_command, count, "no unbounded automatic replacement");
            context("authority");
            c.resume_plan(plan.clone());
            let replacement = c.plans.get(&word(&plan)).unwrap().closures[0];
            assert_ne!(replacement, closure);
            assert_eq!(c.commands.get(&closure).unwrap().command.hex(), original);
            assert_eq!(c.commands.get(&closure).unwrap().state, terminal);
            assert_eq!(c.domain(uint(1)).pending_reserves, 0);
            assert_eq!(c.domain(uint(1)).free, 40);
            context("authority");
            c.resume_plan(plan.clone());
            assert_eq!(
                c.next_command,
                count + 1,
                "unknown replacement stays intact"
            );
            ack(
                &mut c,
                number(replacement),
                7,
                if allocated { 40 } else { 100 },
            );
            context("relayer");
            c.advance(plan.clone());
            assert_eq!(
                c.get_plan(plan)["status"],
                if allocated { "Funded" } else { "Closed" }
            );
            drop(c);
            near_sdk::mock::with_mocked_blockchain(|blockchain| {
                blockchain.take_storage();
            });
        }
    }
}

#[test]
#[should_panic(expected = "NOT_FIFO_HEAD")]
fn cannot_bypass_large_fifo_head() {
    let mut c = fixture();
    request(&mut c, 0, 200);
    request(&mut c, 1, 10);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.approve(hex(&uint(1)), hex(&uint(1)));
    c.apply(hex(&uint(1)), hex(&uint(1)), 10, vec![], 1000);
}

#[test]
#[should_panic(expected = "ONLY_AUTHORITY")]
fn owner_is_not_implicitly_authority() {
    let mut c = fixture();
    request(&mut c, 0, 10);
    context("owner");
    c.approve(hex(&uint(1)), hex(&uint(0)));
}

#[test]
#[should_panic(expected = "INSUFFICIENT_SOURCE_FUNDS_INCLUDING_FEES")]
fn source_must_cover_fee_and_buffer() {
    let mut c = fixture();
    request(&mut c, 0, 100);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.apply(
        hex(&uint(1)),
        hex(&uint(0)),
        0,
        vec![RouteInput {
            route: hex(&uint(9)),
            source: hex(&uint(2)),
            input: 101,
            min_input: 100,
            output: 100,
            deposit: hex(&uint(4)),
            refund: "refund".into(),
            recipient: format!("r{}.project.test.near", &hex(&uint(9))[..24]),
            quote_hash: hex(&uint(7)),
            expires: 1000,
        }],
        1000,
    );
}

#[test]
fn independent_plans_and_owner_withdrawal_reconcile_reservations_out_of_order() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    request(&mut c, 1, 30);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.approve(hex(&uint(1)), hex(&uint(1)));
    let first = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    assert_eq!(c.available(uint(1)), 40);
    ack(&mut c, 1, 5, 40);
    context("relayer");
    c.advance(first);
    // The earlier plan is still allocating. Its proven source reservation lets
    // the independent next FIFO entry commit only the remaining free cash.
    context("authority");
    c.apply(hex(&uint(1)), hex(&uint(1)), 30, vec![], 1000);
    assert_eq!(c.domain(uint(1)).pending_reserves, 30);
    assert_eq!(c.available(uint(1)), 10);
    context("owner");
    let withdrawal = c.withdraw_free(hex(&uint(1)), 10, 1000);
    assert_eq!(c.domain(uint(1)).pending_reserves, 40);
    assert_eq!(c.available(uint(1)), 0);

    let mut latest = snapshot(1, 8, 0);
    latest.words[6] = uint(90);
    c.consume(latest);
    // An aggregate already includes both effects, but their specific immutable
    // acknowledgements arrive in reverse order and with older sequences.
    ack(&mut c, number(word(&withdrawal)), 7, 0);
    assert_eq!(c.domain(uint(1)).pending_reserves, 30);
    ack(&mut c, 3, 6, 10);
    ack(&mut c, 3, 6, 10);
    assert_eq!(c.domain(uint(1)).pending_reserves, 0);
    assert_eq!(c.domain(uint(1)).sequence, 8);
    assert_eq!(c.domain(uint(1)).held, 90);
    assert_eq!(c.domain(uint(1)).free, 0);
    assert_eq!(c.available(uint(1)), 0);
}

#[test]
fn invoice_intention_is_separate_from_once_only_actual_funding() {
    let mut c = fixture();
    context("authority");
    let invoice = c.open_invoice(100, hex(&uint(1)));
    context("owner");
    c.respond_invoice(invoice, 0, None);
    let payload = hex(&[
        uint(1),
        uint(1),
        uint(invoice),
        uint(60),
        hash(b"owner.test.near"),
    ]
    .concat());
    c.consume_funding(uint(1), &payload);
    c.consume_funding(uint(1), &payload);
    assert_eq!(c.get_invoice(invoice)["intended"], 0);
    assert_eq!(c.get_invoice(invoice)["actual_funding"], 60);
    context("authority");
    let partial = c.open_invoice(100, hex(&uint(1)));
    context("owner");
    c.respond_invoice(partial, 30, Some(hex(&uint(2))));
    c.consume_funding(
        uint(1),
        &hex(&[
            uint(1),
            uint(2),
            uint(partial),
            uint(10),
            hash(b"owner.test.near"),
        ]
        .concat()),
    );
    assert_eq!(c.get_invoice(partial)["intended"], 30);
    assert_eq!(c.get_invoice(partial)["actual_funding"], 10);
}

#[test]
fn abort_tombstone_releases_unexecuted_reservation_only_after_ack() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    c.abort(plan.clone());
    assert_eq!(c.domain(uint(1)).pending_reserves, 60);
    let r = c.commands.get(&uint(2)).unwrap();
    let mut p = snapshot(1, 5, 100);
    p.words[32] = uint(2);
    p.words[33] = r.command.digest();
    p.words[34] = uint(15);
    p.words[35] = uint(1);
    p.words[36] = uint(3);
    c.consume(p);
    assert_eq!(c.domain(uint(1)).pending_reserves, 0);
    assert_eq!(c.commands.get(&uint(1)).unwrap().state, 3);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan)["status"], "Closed");
}

#[test]
fn requeue_keeps_original_approval_priority_after_safe_abort() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    request(&mut c, 1, 20);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.approve(hex(&uint(1)), hex(&uint(1)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    ack(&mut c, 1, 5, 40);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.head, 1);
    context("authority");
    c.abort(plan.clone());
    let invalidate = c.commands.get(&uint(3)).unwrap();
    let mut p = snapshot(1, 6, 40);
    p.words[32] = uint(3);
    p.words[33] = invalidate.command.digest();
    p.words[34] = uint(15);
    p.words[35] = uint(1);
    p.words[36] = uint(3);
    c.consume(p);
    context("relayer");
    c.advance(plan.clone());
    ack(&mut c, 4, 7, 100);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan.clone())["status"], "Closed");
    context("authority");
    c.requeue(plan);
    assert_eq!(c.head, 0);
    assert_eq!(
        c.get_request(hex(&uint(1)), hex(&uint(0)))["status"],
        "Queued"
    );
}

#[test]
fn abort_reconciles_already_allocated_plan_without_releasing_agent_rights() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    ack(&mut c, 1, 4, 40);
    context("relayer");
    c.advance(plan.clone());
    context("authority");
    c.abort(plan.clone());
    let mut proof = snapshot(1, 5, 40);
    proof.words[23] = uint(1);
    proof.words[24] = uint(2);
    c.consume(proof);
    assert_eq!(c.get_plan(plan.clone())["status"], "Settling");
    context("relayer");
    c.advance(plan.clone());
    assert!(!c.plans.get(&word(&plan)).unwrap().closures.is_empty());
    assert_eq!(
        c.get_request(hex(&uint(1)), hex(&uint(0)))["status"],
        "Funded"
    );
}

#[test]
fn restoration_confirmation_joins_fifo_once_and_retry_never_creates_payment() {
    let mut c = fixture();
    let mut task = snapshot(1, 2, 100);
    task.words[7] = uint(0);
    task.words[8] = uint(0);
    task.words[9] = uint(1);
    task.words[12] = uint(60);
    c.consume(task);
    context("authority");
    let op = c.restore(hex(&uint(1)), hex(&uint(0)), 30, hex(&uint(9)), 1000);
    let record = c.commands.get(&word(&op)).unwrap();
    let mut proof = snapshot(1, 3, 100);
    proof.words[32] = word(&op);
    proof.words[33] = record.command.digest();
    proof.words[34] = uint(13);
    proof.words[35] = uint(1);
    proof.words[36] = uint(7);
    proof.words[7] = uint(0);
    proof.words[8] = uint(0);
    proof.words[9] = uint(1);
    proof.words[12] = uint(60);
    proof.words[37] = uint(7);
    proof.words[38] = uint(1);
    c.consume(proof.clone());
    c.consume(proof);
    assert_eq!(c.queue.len(), 1);
    assert_eq!(
        c.get_request(hex(&uint(1)), hex(&uint(7)))["status"],
        "Queued"
    );
    context("authority");
    let plan = c.apply(hex(&uint(1)), hex(&uint(7)), 30, vec![], 1000);
    let commands = c.next_command;
    for _ in 0..3 {
        context("authority");
        c.retry(
            plan.clone(),
            Some("RPC timeout".into()),
            Some("reserve outcome".into()),
        );
    }
    assert_eq!(c.next_command, commands);
    assert_eq!(c.domain(uint(1)).pending_reserves, 30);
    assert_eq!(c.get_plan(plan.clone())["requires_attention"], true);
    context("authority");
    c.resume_plan(plan.clone());
    assert_eq!(c.get_plan(plan)["requires_attention"], false);
    assert_eq!(c.next_command, commands);
}

#[test]
fn actual_buffer_refund_is_independent_of_grant_and_all_plan_holds_close() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    let route = RouteInput {
        route: hex(&uint(9)),
        source: hex(&uint(2)),
        input: 65,
        min_input: 62,
        output: 60,
        deposit: hex(&uint(4)),
        refund: "refund".into(),
        recipient: format!("r{}.project.test.near", &hex(&uint(9))[..24]),
        quote_hash: hex(&uint(7)),
        expires: 1000,
    };
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 0, vec![route], 1000);
    ack(&mut c, 1, 4, 100);
    ack(&mut c, 2, 2, 35);
    context("relayer");
    c.advance(plan.clone());
    ack(&mut c, 3, 5, 100);
    ack(&mut c, 4, 3, 35);
    context("relayer");
    c.advance(plan.clone());
    ack(&mut c, 5, 4, 35);
    let mut delivered = snapshot(1, 6, 100);
    delivered.words[26] = uint(9);
    delivered.words[27] = uint(1);
    delivered.words[28] = uint(1);
    delivered.words[29] = uint(60);
    delivered.words[30] = uint(60);
    c.consume(delivered);
    let mut source = snapshot(2, 5, 36);
    source.words[26] = uint(9);
    source.words[27] = uint(1);
    source.words[28] = uint(2);
    source.words[29] = uint(65);
    source.words[30] = uint(1);
    source.words[31] = uint(65);
    c.consume(source.clone());
    context("relayer");
    c.advance(plan.clone());
    assert!(c.get_plan(plan.clone())["routes"][0]["confirmed_transport_expense"].is_null());
    ack(&mut c, 6, 7, 100);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan.clone())["status"], "Settling");
    assert_eq!(c.plans.get(&word(&plan)).unwrap().closures.len(), 2);
    ack(&mut c, 7, 8, 100);
    ack(&mut c, 8, 6, 36);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan.clone())["status"], "Funded");
    source.words[2] = uint(7);
    source.words[5] = uint(38);
    source.words[30] = uint(3);
    c.consume(source);
    assert_eq!(
        c.get_plan(plan.clone())["routes"][0]["confirmed_transport_expense"],
        2
    );
    assert_eq!(c.get_plan(plan)["routes"][0]["unsettled_input"], 0);
    assert_eq!(c.domain(uint(2)).free, 38);
}

#[test]
fn immutable_pending_invalidation_waits_for_independent_terminal_dispatch_evidence() {
    for terminal in [1, 2] {
        // Separate prefixes avoid sharing persistent fixture state inside one mocked runtime.
        let mut c = fixture();
        request(&mut c, 0, 60);
        context("authority");
        c.approve(hex(&uint(1)), hex(&uint(0)));
        let plan = c.apply(
            hex(&uint(1)),
            hex(&uint(0)),
            0,
            vec![RouteInput {
                route: hex(&uint(9)),
                source: hex(&uint(2)),
                input: 60,
                min_input: 60,
                output: 60,
                deposit: hex(&uint(4)),
                refund: "refund".into(),
                recipient: format!("r{}.project.test.near", &hex(&uint(9))[..24]),
                quote_hash: hex(&uint(7)),
                expires: 1000,
            }],
            1000,
        );
        ack(&mut c, 1, 4, 100);
        ack(&mut c, 2, 2, 40);
        context("relayer");
        c.advance(plan.clone());
        ack(&mut c, 3, 5, 100);
        ack(&mut c, 4, 3, 40);
        context("relayer");
        c.advance(plan.clone());
        let dispatch = c.commands.get(&uint(5)).unwrap();
        let mut pending = snapshot(2, 4, 40);
        pending.words[32] = uint(5);
        pending.words[33] = dispatch.command.digest();
        pending.words[34] = uint(4);
        pending.words[35] = uint(4);
        c.consume(pending.clone());
        context("authority");
        c.abort(plan.clone());
        let invalidation = c.commands.get(&uint(6)).unwrap();
        let mut proof = snapshot(2, 5, 40);
        proof.words[32] = uint(6);
        proof.words[33] = invalidation.command.digest();
        proof.words[34] = uint(15);
        proof.words[35] = uint(1);
        proof.words[36] = uint(4);
        c.consume(proof);
        context("relayer");
        c.advance(plan.clone());
        assert!(c.plans.get(&word(&plan)).unwrap().closures.is_empty());
        pending.words[2] = uint(6);
        pending.words[35] = uint(terminal);
        pending.words[36] = uint(if terminal == 1 { 60 } else { 0 });
        c.consume(pending);
        context("relayer");
        c.advance(plan.clone());
        assert_eq!(c.plans.get(&word(&plan)).unwrap().closures.len(), 2);
        assert_eq!(c.commands.get(&uint(6)).unwrap().result, uint(4));
        ack(&mut c, 7, 7, 100);
        ack(&mut c, 8, 8, if terminal == 1 { 40 } else { 100 });
        context("relayer");
        c.advance(plan.clone());
        assert_eq!(c.get_plan(plan.clone())["status"], "Closed");
        if terminal == 1 {
            let mut refund = snapshot(2, 9, 97);
            refund.words[26] = uint(9);
            refund.words[27] = uint(1);
            refund.words[28] = uint(2);
            refund.words[29] = uint(60);
            refund.words[30] = uint(57);
            refund.words[31] = uint(60);
            c.consume(refund);
        }
        context("authority");
        c.requeue(plan.clone());
        assert_eq!(c.head, 0);
        assert!(c.get_plan(plan.clone())["routes"][0]["confirmed_transport_expense"].is_null());
        assert_eq!(
            c.get_plan(plan)["routes"][0]["unsettled_input"],
            if terminal == 1 { 3 } else { 0 }
        );
        assert_eq!(c.available(uint(2)), if terminal == 1 { 97 } else { 100 });
        drop(c);
        near_sdk::mock::with_mocked_blockchain(|blockchain| {
            blockchain.take_storage();
        });
    }
}

#[test]
fn invalid_provider_callback_clears_signing_lock_and_allows_retry() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    let fixtures: Value = near_sdk::serde_json::from_str(include_str!(
        "../../project/tests/fixtures/coordinated-signatures.json"
    ))
    .unwrap();
    let raw = fixtures["reserve"]["signature"].as_str().unwrap();
    let wrong_key = json!({
        "scheme": "Secp256k1",
        "big_r": {"affine_point": format!("03{}", &raw[..64])},
        "s": {"scalar": &raw[64..]},
        "recovery_id": 1
    });
    let malformed_hex = json!({
        "scheme": "Secp256k1",
        "big_r": {"affine_point": "zz".repeat(33)},
        "s": {"scalar": "11".repeat(32)},
        "recovery_id": 0
    });
    for (result, error) in [
        (near_sdk::PromiseResult::Failed, "SIGNATURE_PROVIDER_FAILED"),
        (
            near_sdk::PromiseResult::Successful(b"not-json".to_vec()),
            "MALFORMED_SIGNATURE_RESPONSE",
        ),
        (
            near_sdk::PromiseResult::Successful(b"null".to_vec()),
            "MALFORMED_SIGNATURE_RESPONSE",
        ),
        (
            near_sdk::PromiseResult::Successful(wrong_key.to_string().into_bytes()),
            "INVALID_PROVIDER_SIGNATURE",
        ),
        (
            near_sdk::PromiseResult::Successful(malformed_hex.to_string().into_bytes()),
            "INVALID_PROVIDER_SIGNATURE",
        ),
    ] {
        context("relayer");
        let _ = c.request_signature(hex(&uint(1)));
        assert!(c.commands.get(&uint(1)).unwrap().signing);
        testing_env!(
            VMContextBuilder::new()
                .current_account_id("coordinator.test.near".parse().unwrap())
                .predecessor_account_id("coordinator.test.near".parse().unwrap())
                .account_balance(NearToken::from_near(100))
                .prepaid_gas(Gas::from_tgas(300))
                .build(),
            near_sdk::test_vm_config(),
            near_sdk::RuntimeFeesConfig::test(),
            Default::default(),
            vec![result]
        );
        assert!(!c.on_signature(hex(&uint(1))));
        let record = c.commands.get(&uint(1)).unwrap();
        assert!(!record.signing);
        assert!(record.signature.is_none());
        assert_eq!(record.signature_error.as_deref(), Some(error));
    }
    context("relayer");
    let _ = c.request_signature(hex(&uint(1)));
    assert!(c.commands.get(&uint(1)).unwrap().signing);
}

#[test]
#[should_panic(expected = "CLOSE_REQUIRES_PLAN_STATE")]
fn close_command_without_bound_plan_state_cannot_acknowledge_closure() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    ack(&mut c, 1, 4, 40);
    context("relayer");
    c.advance(plan.clone());
    ack(&mut c, 2, 5, 40);
    context("relayer");
    c.advance(plan);
    let close = c.commands.get(&uint(3)).unwrap();
    let mut proof = snapshot(1, 6, 40);
    proof.words[32] = uint(3);
    proof.words[33] = close.command.digest();
    proof.words[34] = uint(6);
    proof.words[35] = uint(1);
    c.consume(proof);
}

#[test]
fn allocated_close_proof_resolves_abort_even_without_allocation_receipt() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    let plan = c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
    ack(&mut c, 1, 4, 40);
    context("relayer");
    c.advance(plan.clone());
    context("authority");
    c.abort(plan.clone());
    let invalidation = c.commands.get(&uint(3)).unwrap();
    let mut proof = snapshot(1, 5, 40);
    proof.words[32] = uint(3);
    proof.words[33] = invalidation.command.digest();
    proof.words[34] = uint(15);
    proof.words[35] = uint(1);
    proof.words[36] = uint(1);
    c.consume(proof);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.commands.get(&uint(2)).unwrap().state, 0);
    let close = c.commands.get(&uint(4)).unwrap();
    let mut proof = snapshot(1, 6, 40);
    proof.words[23] = uint(1);
    proof.words[24] = uint(2);
    proof.words[32] = uint(4);
    proof.words[33] = close.command.digest();
    proof.words[34] = uint(6);
    proof.words[35] = uint(1);
    c.consume(proof);
    context("relayer");
    c.advance(plan.clone());
    assert_eq!(c.get_plan(plan)["status"], "Funded");
    assert_eq!(
        c.requests
            .get(&identity(uint(1), uint(0)))
            .unwrap()
            .local_state,
        2
    );
}

#[test]
fn queue_tombstone_progress_is_bounded_and_persisted() {
    let mut c = fixture();
    for id in 0..205 {
        c.queue.push(uint(id));
    }
    assert_eq!(c.advance_queue(), 100);
    assert_eq!(c.advance_queue(), 200);
    assert_eq!(c.advance_queue(), 205);
    assert_eq!(c.advance_queue(), 205);
}

#[test]
fn task_only_cancel_removes_old_request_without_stale_revival() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    request(&mut c, 1, 20);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    c.approve(hex(&uint(1)), hex(&uint(1)));
    context("owner");
    let operation = c.control(hex(&uint(1)), hex(&uint(0)), 12, 0, 1000);
    assert!(!c.request_available(c.requests.get(&identity(uint(1), uint(0))).unwrap()));
    let command = c.commands.get(&word(&operation)).unwrap();
    let mut proof = snapshot(1, 5, 100);
    proof.words[7] = uint(0);
    proof.words[8] = uint(4);
    proof.words[32] = word(&operation);
    proof.words[33] = command.command.digest();
    proof.words[34] = uint(12);
    proof.words[35] = uint(1);
    c.consume(proof);
    request(&mut c, 0, 60); // An older valid request/task proof cannot revive cancelled work.
    assert_eq!(
        c.get_request(hex(&uint(1)), hex(&uint(0)))["local_state"],
        4
    );
    assert_eq!(c.get_task(hex(&uint(1)), hex(&uint(0)))["state"], 4);
    context("authority");
    c.apply(hex(&uint(1)), hex(&uint(1)), 20, vec![], 1000);
    assert_eq!(c.head, 1);
}

#[test]
#[should_panic(expected = "TASK_REQUEST_NOT_AVAILABLE")]
fn unconfirmed_owner_control_blocks_new_plan() {
    let mut c = fixture();
    request(&mut c, 0, 60);
    context("authority");
    c.approve(hex(&uint(1)), hex(&uint(0)));
    context("owner");
    c.control(hex(&uint(1)), hex(&uint(0)), 12, 0, 1000);
    context("authority");
    c.apply(hex(&uint(1)), hex(&uint(0)), 60, vec![], 1000);
}
