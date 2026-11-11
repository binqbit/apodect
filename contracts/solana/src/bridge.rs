//! Fixed cross-chain wire records and bounded PDA persistence shared by bridge instructions.
use anchor_lang::prelude::*;
use anchor_lang::{system_program, AccountDeserialize, AccountSerialize, Discriminator};

use crate::errors::ContractError;

pub const COMMAND_BYTES: usize = 384;
pub const CHECKPOINT_BYTES: usize = 1248;
pub const OPEN: u8 = 1;
pub const ALLOCATED: u8 = 2;
pub const CLOSED: u8 = 3;
pub const DELIVERY: u8 = 1;
pub const REFUND: u8 = 2;
pub const SUCCEEDED: u8 = 1;
pub const INVALIDATED: u8 = 3;

/// Stable action numbers in the shared 384-byte command format.
pub mod action {
    pub const RESERVE: u8 = 1;
    pub const EXPECT_DELIVERY: u8 = 2;
    pub const EXPECT_REFUND: u8 = 3;
    pub const DISPATCH: u8 = 4;
    pub const ALLOCATE: u8 = 5;
    pub const CLOSE_PLAN: u8 = 6;
    pub const REJECT: u8 = 7;
    pub const OUT_OF_BUDGET: u8 = 8;
    pub const PAUSE: u8 = 9;
    pub const RECLAIM: u8 = 10;
    pub const CONTINUE: u8 = 11;
    pub const CANCEL: u8 = 12;
    pub const RESTORE_REQUEST: u8 = 13;
    pub const OWNER_WITHDRAW: u8 = 14;
    pub const INVALIDATE: u8 = 15;
}

#[account]
#[derive(InitSpace)]
pub struct BridgePlan {
    pub project: Pubkey,
    pub id: [u8; 32],
    pub state: u8,
    pub held: u64,
}

#[account]
#[derive(InitSpace)]
pub struct BridgeRoute {
    pub project: Pubkey,
    pub id: [u8; 32],
    pub plan: [u8; 32],
    pub kind: u8,
    pub expected: u64,
    pub received: u64,
    pub dispatched: u64,
}

#[account]
#[derive(InitSpace)]
pub struct CommandReceipt {
    pub project: Pubkey,
    pub operation: [u8; 32],
    pub digest: [u8; 32],
    pub action: u8,
    pub state: u8,
    pub result: [u8; 32],
}

/// Immutable evidence: exactly the common payload after the Anchor discriminator.
#[account]
pub struct Checkpoint {
    pub payload: [u8; CHECKPOINT_BYTES],
}

#[account]
pub struct FundingReceipt {
    pub payload: [u8; 160],
}

#[derive(Clone)]
pub struct Command {
    pub domain: [u8; 32],
    pub nonce: u64,
    pub action: u8,
    pub operation: [u8; 32],
    pub related: [u8; 32],
    pub task: [u8; 32],
    pub request: [u8; 32],
    pub amount: u64,
    pub recipient: [u8; 32],
    pub expires: u64,
    pub reference: [u8; 32],
}

pub fn word(value: u64) -> [u8; 32] {
    let mut result = [0; 32];
    result[24..].copy_from_slice(&value.to_be_bytes());
    result
}

pub fn integer(value: &[u8]) -> Result<u64> {
    require!(
        value.len() == 32 && value[..24] == [0; 24],
        ContractError::InvalidCommand
    );
    Ok(u64::from_be_bytes(value[24..].try_into().unwrap()))
}

impl Command {
    pub fn parse(bytes: &[u8; COMMAND_BYTES]) -> Result<Self> {
        let words: Vec<[u8; 32]> = bytes
            .chunks_exact(32)
            .map(|w| w.try_into().unwrap())
            .collect();
        let action = integer(&words[2])?;
        require!(
            (u64::from(action::RESERVE)..=u64::from(action::INVALIDATE)).contains(&action)
                && words[3] != [0; 32],
            ContractError::InvalidCommand
        );
        let action = action as u8;
        // Enforce every reserved/unused word, rather than accepting ambiguous signed encodings.
        let allowed: &[usize] = match action {
            action::RESERVE => &[4, 7],
            action::EXPECT_DELIVERY | action::EXPECT_REFUND => &[4, 7, 11],
            action::DISPATCH => &[4, 7, 9, 11],
            action::ALLOCATE => &[4, 5, 6, 7],
            action::CLOSE_PLAN => &[4],
            action::REJECT => &[5, 6],
            action::OUT_OF_BUDGET => &[5],
            action::PAUSE | action::CONTINUE => &[1, 5],
            action::RECLAIM => &[1, 5, 7],
            action::CANCEL => &[1, 5],
            action::RESTORE_REQUEST => &[5, 7, 11],
            action::OWNER_WITHDRAW => &[7, 9],
            action::INVALIDATE => &[11],
            _ => unreachable!(),
        };
        for i in [1, 4, 5, 6, 7, 8, 9, 11] {
            require!(
                allowed.contains(&i) || words[i] == [0; 32],
                ContractError::InvalidCommand
            );
        }
        let result = Self {
            domain: words[0],
            nonce: integer(&words[1])?,
            action,
            operation: words[3],
            related: words[4],
            task: words[5],
            request: words[6],
            amount: integer(&words[7])?,
            recipient: words[9],
            expires: integer(&words[10])?,
            reference: words[11],
        };
        require!(
            !(action::RESERVE..=action::CLOSE_PLAN).contains(&action) || result.related != [0; 32],
            ContractError::InvalidCommand
        );
        require!(
            !(action == action::ALLOCATE
                || (action::REJECT..=action::RESTORE_REQUEST).contains(&action))
                || result.task != [0; 32],
            ContractError::InvalidCommand
        );
        require!(
            !matches!(action, action::ALLOCATE | action::REJECT) || result.request != [0; 32],
            ContractError::InvalidCommand
        );
        require!(
            !matches!(
                action,
                action::EXPECT_DELIVERY
                    | action::EXPECT_REFUND
                    | action::DISPATCH
                    | action::RESTORE_REQUEST
                    | action::INVALIDATE
            ) || result.reference != [0; 32],
            ContractError::InvalidCommand
        );
        require!(
            !matches!(
                action,
                action::EXPECT_DELIVERY
                    | action::EXPECT_REFUND
                    | action::DISPATCH
                    | action::ALLOCATE
                    | action::RECLAIM
                    | action::RESTORE_REQUEST
                    | action::OWNER_WITHDRAW
            ) || result.amount > 0,
            ContractError::ZeroAmount
        );
        require!(
            !matches!(action, action::DISPATCH | action::OWNER_WITHDRAW)
                || result.recipient != [0; 32],
            ContractError::InvalidCommand
        );
        Ok(result)
    }

    pub fn authenticate(
        &self,
        bytes: &[u8; COMMAND_BYTES],
        signature: &[u8; 64],
        recovery: u8,
        key: &[u8; 64],
    ) -> Result<[u8; 32]> {
        const HALF_ORDER: [u8; 32] = [
            0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46,
            0x68, 0x1b, 0x20, 0xa0,
        ];
        require!(
            recovery <= 1
                && signature[..32] != [0; 32]
                && signature[32..] != [0; 32]
                && signature[32..] <= HALF_ORDER[..],
            ContractError::InvalidSignature
        );
        let digest = solana_sha256_hasher::hash(bytes).to_bytes();
        let recovered = solana_secp256k1_recover::secp256k1_recover(&digest, recovery, signature)
            .map_err(|_| error!(ContractError::InvalidSignature))?;
        require!(
            recovered.to_bytes() == *key,
            ContractError::InvalidSignature
        );
        Ok(digest)
    }
}

pub fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &crate::ID).0
}

pub(crate) fn check_pda(account: &AccountInfo, seeds: &[&[u8]]) -> Result<()> {
    require_keys_eq!(*account.key, pda(seeds), ContractError::WrongBridgeAccount);
    Ok(())
}

pub(crate) fn load<T: AccountDeserialize>(account: &AccountInfo) -> Result<Option<T>> {
    if account.owner == &system_program::ID && account.data_is_empty() {
        return Ok(None);
    }
    require_keys_eq!(*account.owner, crate::ID, ContractError::WrongBridgeAccount);
    Ok(Some(T::try_deserialize(
        &mut account.try_borrow_data()?.as_ref(),
    )?))
}

pub(crate) fn save<T: AccountSerialize>(account: &AccountInfo, value: &T) -> Result<()> {
    require_keys_eq!(*account.owner, crate::ID, ContractError::WrongBridgeAccount);
    value.try_serialize(&mut account.try_borrow_mut_data()?.as_mut())
}

/// Supports pre-funded PDAs without permitting reinitialization of existing program state.
pub(crate) fn create_pda<'info>(
    account: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
    size: usize,
    owner: Pubkey,
) -> Result<()> {
    check_pda(account, seeds)?;
    require!(
        account.owner == &system_program::ID && account.data_is_empty(),
        ContractError::WrongBridgeAccount
    );
    let (_, bump) = Pubkey::find_program_address(seeds, &crate::ID);
    let bump_bytes = [bump];
    let mut signer = seeds.to_vec();
    signer.push(&bump_bytes);
    let rent = Rent::get()?.minimum_balance(size);
    let top_up = rent.saturating_sub(account.lamports());
    if top_up > 0 {
        system_program::transfer(
            CpiContext::new(
                system.key(),
                system_program::Transfer {
                    from: payer.clone(),
                    to: account.clone(),
                },
            ),
            top_up,
        )?;
    }
    system_program::allocate(
        CpiContext::new_with_signer(
            system.key(),
            system_program::Allocate {
                account_to_allocate: account.clone(),
            },
            &[&signer],
        ),
        size as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            system.key(),
            system_program::Assign {
                account_to_assign: account.clone(),
            },
            &[&signer],
        ),
        &owner,
    )?;
    Ok(())
}

pub(crate) fn create_record<'info, T: AccountSerialize + Discriminator>(
    account: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
    space: usize,
    value: &T,
) -> Result<()> {
    create_pda(account, payer, system, seeds, space, crate::ID)?;
    save(account, value)
}
