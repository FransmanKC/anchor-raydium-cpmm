//! Test only. Plays an attacker's program: calls the program in the first
//! account with this instruction's data, passing every other account with
//! the flags it was given.

use anchor_lang::solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke,
    program_error::ProgramError,
    pubkey::Pubkey,
};

entrypoint!(process);

fn process(_: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (program, rest) = accounts
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    let metas = rest
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer,
            is_writable: a.is_writable,
        })
        .collect();
    invoke(
        &Instruction {
            program_id: *program.key,
            accounts: metas,
            data: data.to_vec(),
        },
        rest,
    )
}
