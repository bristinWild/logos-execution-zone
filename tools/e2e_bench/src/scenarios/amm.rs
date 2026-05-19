//! AMM swap flow: setup two tokens, create pool, swap, add liquidity, remove liquidity.

use std::time::Instant;

use anyhow::{Result, bail};
use integration_tests::public_mention;
use wallet::cli::{
    Command, SubcommandReturnValue,
    account::{AccountSubcommand, NewSubcommand},
    programs::{amm::AmmProgramAgnosticSubcommand, token::TokenProgramAgnosticSubcommand},
};

use crate::harness::{ScenarioOutput, finalize_step};

pub async fn run(ctx: &mut crate::bench_context::BenchContext) -> Result<ScenarioOutput> {
    let mut output = ScenarioOutput::new("amm_swap_flow");

    let def_a = new_public_account(ctx, &mut output, "create_acc_def_a").await?;
    let supply_a = new_public_account(ctx, &mut output, "create_acc_supply_a").await?;
    let user_a = new_public_account(ctx, &mut output, "create_acc_user_a").await?;

    let def_b = new_public_account(ctx, &mut output, "create_acc_def_b").await?;
    let supply_b = new_public_account(ctx, &mut output, "create_acc_supply_b").await?;
    let user_b = new_public_account(ctx, &mut output, "create_acc_user_b").await?;

    let user_lp = new_public_account(ctx, &mut output, "create_acc_user_lp").await?;

    timed_token_new(ctx, &mut output, "token_a_new", def_a, supply_a, "TokA").await?;
    timed_token_send(
        ctx,
        &mut output,
        "token_a_fund_user",
        supply_a,
        user_a,
        1_000,
    )
    .await?;

    timed_token_new(ctx, &mut output, "token_b_new", def_b, supply_b, "TokB").await?;
    timed_token_send(
        ctx,
        &mut output,
        "token_b_fund_user",
        supply_b,
        user_b,
        1_000,
    )
    .await?;

    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::AMM(AmmProgramAgnosticSubcommand::New {
                user_holding_a: public_mention(user_a),
                user_holding_b: public_mention(user_b),
                user_holding_lp: public_mention(user_lp),
                balance_a: 300,
                balance_b: 300,
            }),
        )
        .await?;
        let step = finalize_step("amm_new_pool", started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::AMM(AmmProgramAgnosticSubcommand::SwapExactInput {
                user_holding_a: public_mention(user_a),
                user_holding_b: public_mention(user_b),
                amount_in: 50,
                min_amount_out: 1,
                token_definition: def_a,
            }),
        )
        .await?;
        let step = finalize_step("amm_swap_exact_input", started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::AMM(AmmProgramAgnosticSubcommand::AddLiquidity {
                user_holding_a: public_mention(user_a),
                user_holding_b: public_mention(user_b),
                user_holding_lp: public_mention(user_lp),
                min_amount_lp: 1,
                max_amount_a: 100,
                max_amount_b: 100,
            }),
        )
        .await?;
        let step = finalize_step("amm_add_liquidity", started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::AMM(AmmProgramAgnosticSubcommand::RemoveLiquidity {
                user_holding_a: public_mention(user_a),
                user_holding_b: public_mention(user_b),
                user_holding_lp: public_mention(user_lp),
                balance_lp: 50,
                min_amount_a: 1,
                min_amount_b: 1,
            }),
        )
        .await?;
        let step = finalize_step("amm_remove_liquidity", started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    Ok(output)
}

async fn new_public_account(
    ctx: &mut crate::bench_context::BenchContext,
    output: &mut ScenarioOutput,
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
    output.push(step);
    match ret {
        SubcommandReturnValue::RegisterAccount { account_id } => Ok(account_id),
        other => bail!("expected RegisterAccount, got {other:?}"),
    }
}

async fn timed_token_new(
    ctx: &mut crate::bench_context::BenchContext,
    output: &mut ScenarioOutput,
    label: &str,
    def_id: nssa::AccountId,
    supply_id: nssa::AccountId,
    name: &str,
) -> Result<()> {
    let pre_block = crate::harness::begin_step(ctx).await?;
    let started = Instant::now();
    let ret = wallet::cli::execute_subcommand(
        ctx.wallet_mut(),
        Command::Token(TokenProgramAgnosticSubcommand::New {
            definition_account_id: public_mention(def_id),
            supply_account_id: public_mention(supply_id),
            name: name.to_owned(),
            total_supply: 10_000,
        }),
    )
    .await?;
    let step = finalize_step(label, started, pre_block, &ret, ctx).await?;
    output.push(step);
    Ok(())
}

async fn timed_token_send(
    ctx: &mut crate::bench_context::BenchContext,
    output: &mut ScenarioOutput,
    label: &str,
    from_id: nssa::AccountId,
    to_id: nssa::AccountId,
    amount: u128,
) -> Result<()> {
    let pre_block = crate::harness::begin_step(ctx).await?;
    let started = Instant::now();
    let ret = wallet::cli::execute_subcommand(
        ctx.wallet_mut(),
        Command::Token(TokenProgramAgnosticSubcommand::Send {
            from: public_mention(from_id),
            to: Some(public_mention(to_id)),
            to_npk: None,
            to_vpk: None,
            to_identifier: Some(0),
            amount,
        }),
    )
    .await?;
    let step = finalize_step(label, started, pre_block, &ret, ctx).await?;
    output.push(step);
    Ok(())
}
