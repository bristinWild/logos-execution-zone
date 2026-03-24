//! Withdraw program — demonstrates event emission on both success and failure paths.
//!
//! On success: emits WithdrawSuccess event, updates account balance.
//! On failure: emits InsufficientFunds event, commits events via
//!   write_nssa_outputs_on_failure(), then panics.
//!
//! NOTE: In RISC0_DEV_MODE (no ZK proofs), the failure path events are only
//! recoverable in production mode where the journal persists through guest panics.
//! In dev mode, use the Result-based pattern instead for testability.

use borsh::{BorshDeserialize, BorshSerialize};
use lez_events::emit_event;
use nssa_core::program::{
    AccountPostState, ProgramInput, read_nssa_inputs,
    write_nssa_outputs, write_nssa_outputs_on_failure,
};
use serde::{Deserialize, Serialize};

pub const INSUFFICIENT_FUNDS: u32 = 1;
pub const WITHDRAW_SUCCESS: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Instruction {
    amount: u128,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InsufficientFunds {
    pub requested: u128,
    pub available: u128,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct WithdrawSuccess {
    pub amount: u128,
    pub remaining: u128,
}

fn main() {
    let (
        ProgramInput {
            pre_states,
            instruction: Instruction { amount },
        },
        instruction_data,
    ) = read_nssa_inputs::<Instruction>();

    let [pre_state] = pre_states
        .try_into()
        .unwrap_or_else(|_| panic!("Expected exactly one account"));

    let balance = pre_state.account.balance;

    if balance < amount {
        emit_event(INSUFFICIENT_FUNDS, &InsufficientFunds {
            requested: amount,
            available: balance,
        });
        // Commit events to journal before panicking.
        // In production ZK mode, these are recoverable from the receipt.
        write_nssa_outputs_on_failure();
        panic!("Insufficient funds: requested {amount}, available {balance}");
    }

    let mut post_account = pre_state.account.clone();
    post_account.balance -= amount;

    emit_event(WITHDRAW_SUCCESS, &WithdrawSuccess {
        amount,
        remaining: post_account.balance,
    });

    let post_state = AccountPostState::new(post_account);
    write_nssa_outputs(instruction_data, vec![pre_state], vec![post_state]);
}
