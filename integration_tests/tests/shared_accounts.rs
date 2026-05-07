#![expect(
    clippy::tests_outside_test_module,
    reason = "Integration test file, not inside a #[cfg(test)] module"
)]

//! Shared account integration tests.
//!
//! Demonstrates:
//! 1. Group creation and GMS distribution via seal/unseal.
//! 2. Shared regular private account creation via `--for-gms`.
//! 3. Funding a shared account from a public account.
//! 4. Syncing discovers the funded shared account state.

use std::time::Duration;

use anyhow::{Context as _, Result};
use integration_tests::{TIME_TO_WAIT_FOR_BLOCK_SECONDS, TestContext, format_public_account_id};
use log::info;
use tokio::test;
use wallet::cli::{
    Command, SubcommandReturnValue,
    account::{AccountSubcommand, NewSubcommand},
    group::GroupSubcommand,
    programs::native_token_transfer::AuthTransferSubcommand,
};

/// Create a group, create a shared account from it, and verify registration.
#[test]
async fn group_create_and_shared_account_registration() -> Result<()> {
    let mut ctx = TestContext::new().await?;

    // Create a group
    let command = Command::Group(GroupSubcommand::New {
        name: "test-group".to_string(),
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    // Verify group exists
    assert!(
        ctx.wallet()
            .storage()
            .user_data
            .group_key_holder("test-group")
            .is_some()
    );

    // Create a shared regular private account from the group
    let command = Command::Account(AccountSubcommand::New(NewSubcommand::PrivateGms {
        group: "test-group".to_string(),
        label: Some("shared-acc".to_string()),
        pda: false,
        seed: None,
        program_id: None,
    }));

    let result = wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;
    let SubcommandReturnValue::RegisterAccount {
        account_id: shared_account_id,
    } = result
    else {
        anyhow::bail!("Expected RegisterAccount return value");
    };

    // Verify shared account is registered in storage
    let entry = ctx
        .wallet()
        .storage()
        .user_data
        .shared_private_account(&shared_account_id)
        .context("Shared account not found in storage")?;
    assert_eq!(entry.group_label, "test-group");
    assert!(entry.pda_seed.is_none());

    info!("Shared account registered: {shared_account_id}");
    Ok(())
}

/// GMS seal/unseal round-trip: export GMS, re-import under a new name, verify key agreement.
#[test]
async fn group_export_import_key_agreement() -> Result<()> {
    let mut ctx = TestContext::new().await?;

    // Create a group
    let command = Command::Group(GroupSubcommand::New {
        name: "alice-group".to_string(),
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    // Export the GMS
    let holder = ctx
        .wallet()
        .storage()
        .user_data
        .group_key_holder("alice-group")
        .context("Group not found")?;
    let gms_hex = hex::encode(holder.dangerous_raw_gms());

    // Import under a different name (simulating Bob receiving the GMS)
    let command = Command::Group(GroupSubcommand::Import {
        name: "bob-copy".to_string(),
        gms: gms_hex,
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    // Both derive the same keys for the same tag
    let alice_holder = ctx
        .wallet()
        .storage()
        .user_data
        .group_key_holder("alice-group")
        .unwrap();
    let bob_holder = ctx
        .wallet()
        .storage()
        .user_data
        .group_key_holder("bob-copy")
        .unwrap();

    let tag = [42_u8; 32];
    let alice_npk = alice_holder
        .derive_keys_for_shared_account(&tag)
        .generate_nullifier_public_key();
    let bob_npk = bob_holder
        .derive_keys_for_shared_account(&tag)
        .generate_nullifier_public_key();

    assert_eq!(
        alice_npk, bob_npk,
        "Key agreement: same GMS produces same keys"
    );

    info!("Key agreement verified");
    Ok(())
}

/// Fund a shared account from a public account via auth-transfer, then sync.
#[test]
async fn fund_shared_account_from_public() -> Result<()> {
    let mut ctx = TestContext::new().await?;

    // Create group and shared account
    let command = Command::Group(GroupSubcommand::New {
        name: "fund-group".to_string(),
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    let command = Command::Account(AccountSubcommand::New(NewSubcommand::PrivateGms {
        group: "fund-group".to_string(),
        label: None,
        pda: false,
        seed: None,
        program_id: None,
    }));
    let result = wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;
    let SubcommandReturnValue::RegisterAccount {
        account_id: shared_id,
    } = result
    else {
        anyhow::bail!("Expected RegisterAccount return value");
    };

    // Initialize the shared account under auth-transfer
    let command = Command::AuthTransfer(AuthTransferSubcommand::Init {
        account_id: Some(format!("Private/{shared_id}")),
        account_label: None,
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    tokio::time::sleep(Duration::from_secs(TIME_TO_WAIT_FOR_BLOCK_SECONDS)).await;

    // Fund from a public account
    let from_public = ctx.existing_public_accounts()[0];
    let command = Command::AuthTransfer(AuthTransferSubcommand::Send {
        from: Some(format_public_account_id(from_public)),
        from_label: None,
        to: Some(format!("Private/{shared_id}")),
        to_label: None,
        to_npk: None,
        to_vpk: None,
        to_identifier: None,
        amount: 100,
    });
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    tokio::time::sleep(Duration::from_secs(TIME_TO_WAIT_FOR_BLOCK_SECONDS)).await;

    // Sync private accounts
    let command = Command::Account(AccountSubcommand::SyncPrivate);
    wallet::cli::execute_subcommand(ctx.wallet_mut(), command).await?;

    // Verify the shared account was updated
    let entry = ctx
        .wallet()
        .storage()
        .user_data
        .shared_private_account(&shared_id)
        .context("Shared account not found after sync")?;

    info!(
        "Shared account balance after funding: {}",
        entry.account.balance
    );
    assert_eq!(
        entry.account.balance, 100,
        "Shared account should have received 100"
    );

    Ok(())
}
