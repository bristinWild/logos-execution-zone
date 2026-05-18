//! Private chained flow: shielded, deshielded, and private-to-private transfers.

use std::time::Instant;

use anyhow::{Result, bail};
use integration_tests::{private_mention, public_mention};
use wallet::cli::{
    Command, SubcommandReturnValue,
    account::{AccountSubcommand, NewSubcommand},
    programs::token::TokenProgramAgnosticSubcommand,
};

use crate::harness::{ScenarioResult, finalize_step};

pub async fn run(ctx: &mut crate::bench_context::BenchContext) -> Result<ScenarioResult> {
    let mut result = ScenarioResult::new("private_chained_flow");

    let def_id = new_public_account(ctx, &mut result, "create_acc_def").await?;
    let supply_id = new_public_account(ctx, &mut result, "create_acc_supply").await?;
    let public_recipient_id =
        new_public_account(ctx, &mut result, "create_acc_pub_recipient").await?;
    let private_a = new_private_account(ctx, &mut result, "create_acc_priv_a").await?;
    let private_b = new_private_account(ctx, &mut result, "create_acc_priv_b").await?;

    // Mint into public supply.
    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::New {
                definition_account_id: public_mention(def_id),
                supply_account_id: public_mention(supply_id),
                name: "PrivToken".to_owned(),
                total_supply: 1_000_000,
            }),
        )
        .await?;
        let step = finalize_step("token_new_fungible", started, pre_block, &ret, ctx).await?;
        result.push(step);
    }

    // Shielded transfer: public supply -> private_a.
    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::Send {
                from: public_mention(supply_id),
                to: Some(private_mention(private_a)),
                to_npk: None,
                to_vpk: None,
                to_identifier: Some(0),
                amount: 1_000,
            }),
        )
        .await?;
        let step = finalize_step("shielded_transfer", started, pre_block, &ret, ctx).await?;
        result.push(step);
    }

    // Deshielded transfer: private_a -> public_recipient.
    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::Send {
                from: private_mention(private_a),
                to: Some(public_mention(public_recipient_id)),
                to_npk: None,
                to_vpk: None,
                to_identifier: Some(0),
                amount: 100,
            }),
        )
        .await?;
        let step = finalize_step("deshielded_transfer", started, pre_block, &ret, ctx).await?;
        result.push(step);
    }

    // Private-to-private transfer: private_a -> private_b.
    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::Send {
                from: private_mention(private_a),
                to: Some(private_mention(private_b)),
                to_npk: None,
                to_vpk: None,
                to_identifier: Some(0),
                amount: 200,
            }),
        )
        .await?;
        let step = finalize_step("private_to_private", started, pre_block, &ret, ctx).await?;
        result.push(step);
    }

    Ok(result)
}

async fn new_public_account(
    ctx: &mut crate::bench_context::BenchContext,
    result: &mut ScenarioResult,
    label: &str,
) -> Result<nssa::AccountId> {
    let pre_block = crate::harness::begin_step(ctx).await?;
    let started = Instant::now();
    let ret = wallet::cli::execute_subcommand(
        ctx.wallet_mut(),
        Command::Account(AccountSubcommand::New(NewSubcommand::Public {
            cci: None,
            label: None,
        })),
    )
    .await?;
    let step = finalize_step(label, started, pre_block, &ret, ctx).await?;
    result.push(step);
    match ret {
        SubcommandReturnValue::RegisterAccount { account_id } => Ok(account_id),
        other => bail!("expected RegisterAccount, got {other:?}"),
    }
}

async fn new_private_account(
    ctx: &mut crate::bench_context::BenchContext,
    result: &mut ScenarioResult,
    label: &str,
) -> Result<nssa::AccountId> {
    let pre_block = crate::harness::begin_step(ctx).await?;
    let started = Instant::now();
    let ret = wallet::cli::execute_subcommand(
        ctx.wallet_mut(),
        Command::Account(AccountSubcommand::New(NewSubcommand::Private {
            cci: None,
            label: None,
        })),
    )
    .await?;
    let step = finalize_step(label, started, pre_block, &ret, ctx).await?;
    result.push(step);
    match ret {
        SubcommandReturnValue::RegisterAccount { account_id } => Ok(account_id),
        other => bail!("expected RegisterAccount, got {other:?}"),
    }
}
