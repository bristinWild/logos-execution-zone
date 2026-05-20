//! Multi-recipient fanout: one funded supply pays 10 distinct recipients.

use std::time::Instant;

use anyhow::{Result, bail};
use test_fixtures::public_mention;
use wallet::cli::{
    Command, SubcommandReturnValue,
    account::{AccountSubcommand, NewSubcommand},
    programs::token::TokenProgramAgnosticSubcommand,
};

use crate::harness::{ScenarioOutput, finalize_step};

const FANOUT_COUNT: usize = 10;
const AMOUNT_PER_TRANSFER: u128 = 100;

pub async fn run(ctx: &mut test_fixtures::TestContext) -> Result<ScenarioOutput> {
    let mut output = ScenarioOutput::new("multi_recipient_fanout");

    let def_id = new_public_account(ctx, &mut output, "create_acc_def").await?;
    let supply_id = new_public_account(ctx, &mut output, "create_acc_supply").await?;

    {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::New {
                definition_account_id: public_mention(def_id),
                supply_account_id: public_mention(supply_id),
                name: "FanoutToken".to_owned(),
                total_supply: 10_000_000,
            }),
        )
        .await?;
        let step = finalize_step("token_new_fungible", started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    let mut recipients = Vec::with_capacity(FANOUT_COUNT);
    for i in 0..FANOUT_COUNT {
        let id = new_public_account(ctx, &mut output, &format!("create_recipient_{i:02}")).await?;
        recipients.push(id);
    }

    for (i, recipient_id) in recipients.iter().enumerate() {
        let pre_block = crate::harness::begin_step(ctx).await?;
        let started = Instant::now();
        let ret = wallet::cli::execute_subcommand(
            ctx.wallet_mut(),
            Command::Token(TokenProgramAgnosticSubcommand::Send {
                from: public_mention(supply_id),
                to: Some(public_mention(*recipient_id)),
                to_npk: None,
                to_vpk: None,
                to_identifier: Some(0),
                amount: AMOUNT_PER_TRANSFER,
            }),
        )
        .await?;
        let step = finalize_step(format!("transfer_{i:02}"), started, pre_block, &ret, ctx).await?;
        output.push(step);
    }

    Ok(output)
}

async fn new_public_account(
    ctx: &mut test_fixtures::TestContext,
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
