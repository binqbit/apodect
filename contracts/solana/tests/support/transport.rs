use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

#[derive(Clone, Copy)]
pub enum Role {
    Owner,
    Authority,
    Agent,
    Stranger,
}

pub fn ix(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: apodect_solana::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

pub fn send(
    svm: &mut LiteSVM,
    payer: &Keypair,
    instructions: &[Instruction],
    extra: &[&Keypair],
) -> Result<(), String> {
    send_logs(svm, payer, instructions, extra).map(|_| ())
}

pub fn send_logs(
    svm: &mut LiteSVM,
    payer: &Keypair,
    instructions: &[Instruction],
    extra: &[&Keypair],
) -> Result<Vec<String>, String> {
    svm.expire_blockhash();
    let mut signers = vec![payer];

    for signer in extra {
        if signer.pubkey() != payer.pubkey() {
            signers.push(*signer);
        }
    }

    let transaction = Transaction::new(
        &signers,
        Message::new(instructions, Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction)
        .map(|metadata| metadata.logs)
        .map_err(|e| format!("{e:?}"))
}
