//! Step / scenario timing primitives shared across scenarios.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use common::transaction::NSSATransaction;
use sequencer_service_rpc::RpcClient as _;
use serde::Serialize;
use wallet::cli::SubcommandReturnValue;

use crate::bench_context::BenchContext;

const TX_INCLUSION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const TX_INCLUSION_TIMEOUT: Duration = Duration::from_secs(120);

/// Borsh-serialized sizes for one zone block fetched after a step. `block_bytes`
/// is the full Block (header + body + bedrock metadata) and is the closest
/// proxy we have to the L1 payload posted per block. `tx_bytes` is each contained
/// transaction split by variant — this is what the fee model's S_tx slot covers.
#[derive(Debug, Serialize, Clone, Default)]
pub struct BlockSize {
    pub block_id: u64,
    pub block_bytes: usize,
    pub public_tx_bytes: Vec<usize>,
    pub ppe_tx_bytes: Vec<usize>,
    pub deploy_tx_bytes: Vec<usize>,
}

#[derive(Debug, Serialize, Clone)]
pub struct StepResult {
    pub label: String,
    pub submit_ms: f64,
    pub inclusion_ms: Option<f64>,
    pub wallet_sync_ms: Option<f64>,
    pub total_ms: f64,
    pub tx_hash: Option<String>,
    /// Borsh sizes for every zone block produced during this step.
    /// Empty for steps that don't advance the chain (e.g. RegisterAccount).
    pub blocks: Vec<BlockSize>,
}

#[derive(Debug, Serialize, Default)]
pub struct ScenarioResult {
    pub name: String,
    pub setup_ms: f64,
    pub steps: Vec<StepResult>,
    pub total_ms: f64,
    /// Disk sizes (sequencer / indexer / wallet tempdirs) sampled at scenario start.
    pub disk_before: Option<crate::bench_context::DiskSizes>,
    /// Disk sizes sampled at scenario end.
    pub disk_after: Option<crate::bench_context::DiskSizes>,
    /// Bedrock-finality latency: time from final-step inclusion to the indexer
    /// reporting the sequencer tip as L1-finalised. Effectively measures the
    /// sequencer→Bedrock posting + Bedrock finalisation + indexer L1 ingest path.
    /// A value at the timeout (60s) means finalisation did not happen within the bench window.
    pub bedrock_finality_ms: Option<f64>,
}

impl ScenarioResult {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn push(&mut self, step: StepResult) {
        self.total_ms += step.total_ms;
        self.steps.push(step);
    }
}

/// Finish a timed wallet step. Records submit (the time between `started`
/// being captured and `ret` being received) and, if `ret` is a
/// [`SubcommandReturnValue::PrivacyPreservingTransfer`], polls the sequencer
/// for inclusion and records the inclusion latency. Returns a [`StepResult`].
///
/// Usage:
/// ```ignore
/// let started = Instant::now();
/// let ret = wallet::cli::execute_subcommand(ctx.wallet_mut(), cmd).await?;
/// let step = finalize_step("label", started, ret, ctx).await?;
/// ```
/// Begin a timed step. Capture this *before* submitting the wallet operation
/// so we can later subtract it from the post-submit block height to detect
/// when the chain has advanced past the tx's block.
pub async fn begin_step(ctx: &BenchContext) -> Result<u64> {
    Ok(ctx.sequencer_client().get_last_block_id().await?)
}

pub async fn finalize_step(
    label: impl Into<String>,
    started: Instant,
    pre_block_id: u64,
    ret: &SubcommandReturnValue,
    ctx: &mut BenchContext,
) -> Result<StepResult> {
    let label = label.into();
    let submit_ms = started.elapsed().as_secs_f64() * 1_000.0;

    let mut tx_hash_str = None;
    let mut inclusion_ms = None;
    let mut wallet_sync_ms = None;
    let mut blocks: Vec<BlockSize> = Vec::new();

    // For non-account-create steps (anything that produces a tx_hash, or even
    // `Empty` for public Token Send), wait for the chain to advance past the
    // submission block so state is applied before the next step. We use
    // get_last_block_id as the canonical "block has been produced and
    // recorded" signal.
    let should_wait_for_chain = !matches!(ret, SubcommandReturnValue::RegisterAccount { .. });
    if should_wait_for_chain {
        if let SubcommandReturnValue::PrivacyPreservingTransfer { tx_hash } = ret {
            tx_hash_str = Some(format!("{tx_hash}"));
        }
        let started_inclusion = Instant::now();
        wait_for_chain_advance(ctx, pre_block_id, 2).await?;
        inclusion_ms = Some(started_inclusion.elapsed().as_secs_f64() * 1_000.0);

        let started_sync = Instant::now();
        sync_wallet_to_tip(ctx).await?;
        wallet_sync_ms = Some(started_sync.elapsed().as_secs_f64() * 1_000.0);

        // Capture block-byte and per-tx-byte sizes for every block produced
        // during this step. We intentionally capture all blocks, including
        // empty clock-only ticks: the empty-block baseline lets the fee model
        // back out the per-tx contribution.
        let tip = ctx.sequencer_client().get_last_block_id().await?;
        for block_id in (pre_block_id.saturating_add(1))..=tip {
            if let Some(block) = ctx.sequencer_client().get_block(block_id).await? {
                let block_bytes = borsh::to_vec(&block).map_or(0, |v| v.len());
                let mut sz = BlockSize {
                    block_id,
                    block_bytes,
                    public_tx_bytes: Vec::new(),
                    ppe_tx_bytes: Vec::new(),
                    deploy_tx_bytes: Vec::new(),
                };
                for tx in &block.body.transactions {
                    let n = borsh::to_vec(tx).map_or(0, |v| v.len());
                    match tx {
                        NSSATransaction::Public(_) => sz.public_tx_bytes.push(n),
                        NSSATransaction::PrivacyPreserving(_) => sz.ppe_tx_bytes.push(n),
                        NSSATransaction::ProgramDeployment(_) => sz.deploy_tx_bytes.push(n),
                    }
                }
                blocks.push(sz);
            }
        }
    }

    Ok(StepResult {
        label,
        submit_ms,
        inclusion_ms,
        wallet_sync_ms,
        total_ms: started.elapsed().as_secs_f64() * 1_000.0,
        tx_hash: tx_hash_str,
        blocks,
    })
}

/// Wait for `get_last_block_id` to advance by at least `min_blocks` from `from_block_id`.
pub async fn wait_for_chain_advance(
    ctx: &BenchContext,
    from_block_id: u64,
    min_blocks: u64,
) -> Result<()> {
    let target = from_block_id.saturating_add(min_blocks);
    let deadline = Instant::now() + TX_INCLUSION_TIMEOUT;
    loop {
        match ctx.sequencer_client().get_last_block_id().await {
            Ok(current) if current >= target => return Ok(()),
            Ok(_) => {}
            Err(err) => eprintln!("get_last_block_id error (continuing poll): {err:#}"),
        }
        if Instant::now() > deadline {
            bail!(
                "chain did not advance from {from_block_id} to at least {target} within {TX_INCLUSION_TIMEOUT:?}"
            );
        }
        tokio::time::sleep(TX_INCLUSION_POLL_INTERVAL).await;
    }
}

async fn sync_wallet_to_tip(ctx: &mut BenchContext) -> Result<()> {
    let last_block = ctx.sequencer_client().get_last_block_id().await?;
    ctx.wallet_mut().sync_to_block(last_block).await?;
    Ok(())
}

pub fn print_table(result: &ScenarioResult) {
    let label_width = result
        .steps
        .iter()
        .map(|s| s.label.len())
        .max()
        .unwrap_or(0)
        .max("step".len());

    println!(
        "\nScenario: {} (setup {:.1} ms ({:.2}s), total {:.1} ms ({:.2}s))",
        result.name,
        result.setup_ms,
        result.setup_ms / 1_000.0,
        result.total_ms,
        result.total_ms / 1_000.0,
    );
    println!(
        "{:<lw$}  {:>10}  {:>12}  {:>10}  {:>16}",
        "step",
        "submit_ms",
        "inclusion_ms",
        "sync_ms",
        "total_ms (s)",
        lw = label_width,
    );
    println!("{}", "-".repeat(label_width + 62));
    for s in &result.steps {
        let inclusion = s
            .inclusion_ms
            .map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"));
        let sync = s
            .wallet_sync_ms
            .map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"));
        let total = format!("{:.1} ({:.2}s)", s.total_ms, s.total_ms / 1_000.0);
        println!(
            "{:<lw$}  {:>10.1}  {:>12}  {:>10}  {:>16}",
            s.label,
            s.submit_ms,
            inclusion,
            sync,
            total,
            lw = label_width,
        );
    }

    print_size_summary(result);
}

/// Aggregate borsh sizes per scenario: total/mean/min/max block bytes, and
/// per-tx bytes split by variant. Empty if no blocks were captured.
fn print_size_summary(result: &ScenarioResult) {
    let blocks: Vec<&BlockSize> = result.steps.iter().flat_map(|s| s.blocks.iter()).collect();
    if blocks.is_empty() {
        return;
    }

    let block_bytes: Vec<usize> = blocks.iter().map(|b| b.block_bytes).collect();
    let total_block_bytes: usize = block_bytes.iter().sum();
    let mean_block = mean_usize(&block_bytes);
    let min_block = block_bytes.iter().copied().min().unwrap_or(0);
    let max_block = block_bytes.iter().copied().max().unwrap_or(0);

    let public: Vec<usize> = blocks
        .iter()
        .flat_map(|b| b.public_tx_bytes.iter().copied())
        .collect();
    let ppe: Vec<usize> = blocks
        .iter()
        .flat_map(|b| b.ppe_tx_bytes.iter().copied())
        .collect();
    let deploy: Vec<usize> = blocks
        .iter()
        .flat_map(|b| b.deploy_tx_bytes.iter().copied())
        .collect();

    println!(
        "\nBlock + tx size summary ({} blocks captured):",
        blocks.len()
    );
    println!(
        "  block_bytes: total={total_block_bytes}, mean={mean_block}, min={min_block}, max={max_block}",
    );
    print_tx_line("public_tx_bytes      ", &public);
    print_tx_line("ppe_tx_bytes         ", &ppe);
    print_tx_line("deploy_tx_bytes      ", &deploy);
}

fn print_tx_line(label: &str, samples: &[usize]) {
    if samples.is_empty() {
        println!("  {label}: (none)");
        return;
    }
    let total: usize = samples.iter().sum();
    let mean = mean_usize(samples);
    let min = samples.iter().copied().min().unwrap_or(0);
    let max = samples.iter().copied().max().unwrap_or(0);
    println!(
        "  {label}: n={}, total={total}, mean={mean}, min={min}, max={max}",
        samples.len()
    );
}

fn mean_usize(xs: &[usize]) -> usize {
    xs.iter().sum::<usize>().checked_div(xs.len()).unwrap_or(0)
}
