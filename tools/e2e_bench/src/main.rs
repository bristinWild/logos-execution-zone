//! End-to-end LEZ scenario bench.
//!
//! Spins up the full stack (native Bedrock node launched per-scenario via
//! `BedrockHandle` + in-process sequencer + indexer + wallet via
//! `BenchContext`) and drives the wallet through configurable scenarios that
//! mirror real user flows. Times each step and records borsh-serialized
//! block + tx sizes per scenario.
//!
//! Required env vars (no defaults; see `tools/e2e_bench/README.md`):
//!   `LEZ_BEDROCK_BIN`          absolute path to logos-blockchain-node.
//!   `LEZ_BEDROCK_CONFIG_DIR`   directory with node-config.yaml + deployment template.
//!
//! Run examples:
//!   `RISC0_DEV_MODE=1` `cargo run --release -p e2e_bench -- --scenario all`.
//!   `cargo run --release -p e2e_bench -- --scenario amm`.
//!
//! `RISC0_DEV_MODE=1` skips proving and produces latency-only numbers in
//! ~minutes; omitting it produces realistic proving-inclusive numbers but
//! the run takes much longer.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::wildcard_enum_match_arm,
    reason = "Bench tool: stderr/stdout output is the deliverable; small Duration / iterator-sum \
              arithmetic is safe at bench scale; bench scenarios bail loudly on any unexpected \
              return variant, which is preferable to maintaining an exhaustive list in five files."
)]

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use bedrock_handle::BedrockHandle;
use bench_context::BenchContext;
use clap::{Parser, ValueEnum};
use harness::ScenarioOutput;
use serde::Serialize;

mod bedrock_handle;
mod bench_context;
mod harness;
mod scenarios;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ScenarioName {
    Token,
    Amm,
    Fanout,
    Private,
    Parallel,
    All,
}

#[derive(Parser, Debug)]
#[command(about = "End-to-end LEZ scenario bench")]
struct Cli {
    /// Which scenario(s) to run.
    #[arg(long, value_enum, default_value_t = ScenarioName::All)]
    scenario: ScenarioName,

    /// Optional JSON output path. Defaults to `<workspace>/target/e2e_bench.json`.
    #[arg(long)]
    json_out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BenchRunReport {
    risc0_dev_mode: bool,
    scenarios: Vec<ScenarioOutput>,
    total_wall_s: f64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // integration_tests initializes env_logger via a LazyLock, so we leave logger
    // setup to it. Set RUST_LOG=info before running to see logs.

    let cli = Cli::parse();
    let risc0_dev_mode = std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0");

    eprintln!(
        "e2e_bench: scenario={:?}, RISC0_DEV_MODE={}",
        cli.scenario,
        if risc0_dev_mode { "1" } else { "unset/0" }
    );

    let to_run: Vec<ScenarioName> = match cli.scenario {
        ScenarioName::All => vec![
            ScenarioName::Token,
            ScenarioName::Amm,
            ScenarioName::Fanout,
            ScenarioName::Private,
            ScenarioName::Parallel,
        ],
        other => vec![other],
    };

    let overall_started = std::time::Instant::now();
    let mut all_outputs = Vec::with_capacity(to_run.len());

    for name in to_run {
        eprintln!("\n=== running scenario: {name:?} ===");
        {
            let setup_started = std::time::Instant::now();
            // Spawn a fresh Bedrock node for this scenario. Each scenario therefore
            // starts with an empty chain so the indexer never has a backlog from a
            // prior scenario.
            let bedrock = BedrockHandle::launch_fresh()
                .await
                .with_context(|| format!("failed to spawn Bedrock for scenario {name:?}"))?;
            let bedrock_addr_string = format!("{}", bedrock.addr());
            // SAFETY: env::set_var happens before any threaded setup that reads env.
            unsafe {
                std::env::set_var("LEZ_BEDROCK_ADDR", &bedrock_addr_string);
            }

            let mut ctx = BenchContext::new()
                .await
                .with_context(|| format!("failed to setup BenchContext for scenario {name:?}"))?;
            let setup = setup_started.elapsed();
            eprintln!("setup: {:.2}s", setup.as_secs_f64());

            let disk_before = ctx.disk_sizes();
            let mut output = run_scenario(name, setup, &mut ctx).await?;
            output.disk_before = Some(disk_before);
            output.disk_after = Some(ctx.disk_sizes());
            output.bedrock_finality = Some(measure_bedrock_finality(&ctx).await?);
            harness::print_table(&output);
            all_outputs.push(output);

            // ctx and bedrock drop here at end of scope, killing the bedrock child
            // before we sleep so the next iteration can rebind the port.
        }
        // Give Bedrock a moment to shut down before the next scenario.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let total_wall_s = overall_started.elapsed().as_secs_f64();
    eprintln!("\nTotal wall time: {total_wall_s:.1}s");

    let report = BenchRunReport {
        risc0_dev_mode,
        scenarios: all_outputs,
        total_wall_s,
    };

    let out_path = if let Some(p) = cli.json_out {
        p
    } else {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()?;
        let suffix = if risc0_dev_mode { "dev" } else { "prove" };
        workspace_root
            .join("target")
            .join(format!("e2e_bench_{suffix}.json"))
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;
    eprintln!("\nJSON written to {}", out_path.display());

    Ok(())
}

async fn run_scenario(
    name: ScenarioName,
    setup: Duration,
    ctx: &mut BenchContext,
) -> Result<ScenarioOutput> {
    let output = match name {
        ScenarioName::Token => scenarios::token::run(ctx).await?,
        ScenarioName::Amm => scenarios::amm::run(ctx).await?,
        ScenarioName::Fanout => scenarios::fanout::run(ctx).await?,
        ScenarioName::Private => scenarios::private::run(ctx).await?,
        ScenarioName::Parallel => scenarios::parallel::run(ctx).await?,
        ScenarioName::All => unreachable!("dispatched above"),
    };
    Ok(ScenarioOutput { setup, ..output })
}

/// Poll the indexer's L1-finalised block id until it catches up with the
/// sequencer's last block id. This is effectively the sequencer→Bedrock posting
/// plus Bedrock finalisation plus indexer ingest latency.
async fn measure_bedrock_finality(ctx: &BenchContext) -> Result<Duration> {
    use indexer_service_rpc::RpcClient as _;
    use jsonrpsee::ws_client::WsClientBuilder;
    use sequencer_service_rpc::RpcClient as _;

    let indexer_url = format!("ws://{}", ctx.indexer_addr());
    let indexer_ws = WsClientBuilder::default()
        .build(&indexer_url)
        .await
        .context("connect indexer WS")?;
    let sequencer_tip = ctx.sequencer_client().get_last_block_id().await?;

    let timeout = Duration::from_secs(60);
    let started = std::time::Instant::now();
    let poll = async {
        loop {
            match indexer_ws.get_last_finalized_block_id().await {
                Ok(Some(b)) if b >= sequencer_tip => return,
                Ok(_) => {}
                Err(err) => eprintln!("indexer last_synced poll error: {err:#}"),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    if tokio::time::timeout(timeout, poll).await.is_err() {
        eprintln!("indexer did not catch up to {sequencer_tip} within {timeout:?}");
    }
    Ok(started.elapsed())
}
