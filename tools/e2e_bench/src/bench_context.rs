//! `BenchContext`: wires sequencer + indexer + wallet in-process against an
//! externally-running Bedrock node. Mirrors the surface of
//! `integration_tests::TestContext` for the methods the scenarios need
//! (`wallet_mut()`, `sequencer_client()`), but skips the docker setup.
//!
//! The external Bedrock URL defaults to 127.0.0.1:18080 and can be overridden
//! with the `LEZ_BEDROCK_ADDR` env var.

#![allow(
    clippy::arbitrary_source_item_ordering,
    reason = "file is deleted in the docker-compose pivot; ordering churn is wasted work"
)]

use std::{env, net::SocketAddr, path::Path};

use anyhow::{Context as _, Result};
use indexer_service::IndexerHandle;
use test_fixtures::config::{
    SequencerPartialConfig, UrlProtocol, addr_to_url, default_private_accounts_for_wallet,
    default_public_accounts_for_wallet, genesis_from_accounts, indexer_config, sequencer_config,
    wallet_config,
};
use sequencer_service::SequencerHandle;
use sequencer_service_rpc::{SequencerClient, SequencerClientBuilder};
use serde::Serialize;
use tempfile::TempDir;
use wallet::{WalletCore, config::WalletConfigOverrides};

const DEFAULT_BEDROCK_ADDR: &str = "127.0.0.1:18080";

#[expect(
    clippy::partial_pub_fields,
    reason = "Internal TempDirs are kept alive via private fields for RAII; \
              client and wallet are public for scenarios to drive."
)]
pub struct BenchContext {
    pub sequencer_client: SequencerClient,
    pub wallet: WalletCore,
    #[expect(
        dead_code,
        reason = "Retained for parity with TestContext; may be needed later."
    )]
    pub wallet_password: String,
    sequencer_handle: Option<SequencerHandle>,
    indexer_handle: IndexerHandle,
    temp_indexer_dir: TempDir,
    temp_sequencer_dir: TempDir,
    temp_wallet_dir: TempDir,
}

impl BenchContext {
    pub async fn new() -> Result<Self> {
        let bedrock_addr_str =
            env::var("LEZ_BEDROCK_ADDR").unwrap_or_else(|_| DEFAULT_BEDROCK_ADDR.to_owned());
        let bedrock_addr: SocketAddr = bedrock_addr_str
            .parse()
            .with_context(|| format!("invalid LEZ_BEDROCK_ADDR `{bedrock_addr_str}`"))?;

        eprintln!("BenchContext: using external bedrock at {bedrock_addr}");

        let initial_public_accounts = default_public_accounts_for_wallet();
        let initial_private_accounts = default_private_accounts_for_wallet();
        let genesis_transactions =
            genesis_from_accounts(&initial_public_accounts, &initial_private_accounts);
        let sequencer_partial = SequencerPartialConfig::default();

        let temp_indexer_dir = tempfile::tempdir().context("indexer temp dir")?;
        let indexer_cfg = indexer_config(bedrock_addr, temp_indexer_dir.path().to_owned())
            .context("indexer config")?;
        let indexer_handle = indexer_service::run_server(indexer_cfg, 0)
            .await
            .context("indexer run_server")?;

        let temp_sequencer_dir = tempfile::tempdir().context("sequencer temp dir")?;
        let sequencer_cfg = sequencer_config(
            sequencer_partial,
            temp_sequencer_dir.path().to_owned(),
            bedrock_addr,
            genesis_transactions,
        )
        .context("sequencer config")?;
        let sequencer_handle = sequencer_service::run(sequencer_cfg, 0)
            .await
            .context("sequencer run")?;

        let temp_wallet_dir = tempfile::tempdir().context("wallet temp dir")?;
        let mut wallet_cfg = wallet_config(sequencer_handle.addr()).context("wallet config")?;
        // The default 30s poll interval is far too slow for a measurement run;
        // shrink so the wallet sees new blocks within ~1s.
        wallet_cfg.seq_poll_timeout = std::time::Duration::from_secs(1);
        let wallet_cfg_str =
            serde_json::to_string_pretty(&wallet_cfg).context("serialize wallet config")?;
        let wallet_cfg_path = temp_wallet_dir.path().join("wallet_config.json");
        std::fs::write(&wallet_cfg_path, wallet_cfg_str).context("write wallet config")?;
        let storage_path = temp_wallet_dir.path().join("storage.json");
        let password = "bench_pass".to_owned();
        let (mut wallet, _mnemonic) = WalletCore::new_init_storage(
            wallet_cfg_path,
            storage_path,
            Some(WalletConfigOverrides::default()),
            &password,
        )
        .context("wallet init")?;
        // Mirror integration_tests::setup_wallet: import the initial accounts
        // produced above so the wallet can reference them by AccountId in scenarios.
        for (private_key, _balance) in &initial_public_accounts {
            wallet
                .storage_mut()
                .key_chain_mut()
                .add_imported_public_account(private_key.clone());
        }
        for private_account in &initial_private_accounts {
            wallet
                .storage_mut()
                .key_chain_mut()
                .add_imported_private_account(
                    private_account.key_chain.clone(),
                    None,
                    private_account.identifier,
                    nssa::Account::default(),
                );
        }
        wallet
            .store_persistent_data()
            .context("wallet store persistent")?;

        let sequencer_url =
            addr_to_url(UrlProtocol::Http, sequencer_handle.addr()).context("sequencer url")?;
        let sequencer_client = SequencerClientBuilder::default()
            .build(sequencer_url)
            .context("build sequencer client")?;

        Ok(Self {
            sequencer_client,
            wallet,
            wallet_password: password,
            sequencer_handle: Some(sequencer_handle),
            indexer_handle,
            temp_indexer_dir,
            temp_sequencer_dir,
            temp_wallet_dir,
        })
    }

    pub const fn wallet_mut(&mut self) -> &mut WalletCore {
        &mut self.wallet
    }

    pub const fn sequencer_client(&self) -> &SequencerClient {
        &self.sequencer_client
    }

    pub const fn indexer_addr(&self) -> SocketAddr {
        self.indexer_handle.addr()
    }

    /// Recursively-sized bytes on disk for sequencer + indexer + wallet tempdirs.
    pub fn disk_sizes(&self) -> DiskSizes {
        DiskSizes {
            sequencer_bytes: dir_size_bytes(self.temp_sequencer_dir.path()),
            indexer_bytes: dir_size_bytes(self.temp_indexer_dir.path()),
            wallet_bytes: dir_size_bytes(self.temp_wallet_dir.path()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[expect(
    clippy::struct_field_names,
    reason = "The `_bytes` suffix carries the unit and is preserved verbatim in JSON output."
)]
pub struct DiskSizes {
    pub sequencer_bytes: u64,
    pub indexer_bytes: u64,
    pub wallet_bytes: u64,
}

fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0_u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        } else {
            // Sockets, FIFOs, block/char devices: ignore. Symlinks are
            // already followed by `is_file()` / `is_dir()`.
        }
    }
    total
}

impl Drop for BenchContext {
    fn drop(&mut self) {
        if let Some(handle) = self.sequencer_handle.take()
            && !handle.is_healthy()
        {
            eprintln!("BenchContext drop: sequencer handle was unhealthy");
        }
        if !self.indexer_handle.is_healthy() {
            eprintln!("BenchContext drop: indexer handle was unhealthy");
        }
    }
}
