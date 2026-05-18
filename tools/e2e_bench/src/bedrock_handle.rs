//! Manages an external `logos-blockchain-node` process as a child of the bench.
//! Launches a fresh Bedrock instance per scenario so the indexer never has to
//! catch up a large finalization backlog.
//!
//! Required env vars (no defaults — path layouts differ per developer):
//! - `LEZ_BEDROCK_BIN`         — absolute path to the `logos-blockchain-node` binary.
//! - `LEZ_BEDROCK_CONFIG_DIR`  — directory containing `node-config.yaml` and
//!   `deployment-settings.yaml` (template with `PLACEHOLDER_CHAIN_START_TIME`).
//!
//! Optional:
//! - `LEZ_BEDROCK_PORT` (default: 18080)

use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};

pub struct BedrockHandle {
    child: Option<Child>,
    addr: SocketAddr,
    workdir: PathBuf,
}

impl BedrockHandle {
    /// Launch a fresh Bedrock node. Cleans `state/` in the working dir, rewrites
    /// `deployment-settings.yaml` with the current UTC `chain_start_time`, spawns
    /// the binary, and polls the HTTP port until ready.
    pub async fn launch_fresh() -> Result<Self> {
        let bin = env::var("LEZ_BEDROCK_BIN").map_err(|err| {
            anyhow::anyhow!(
                "LEZ_BEDROCK_BIN is required ({err}). Set it to the absolute path of the \
                 logos-blockchain-node binary (e.g. \
                 `export LEZ_BEDROCK_BIN=/path/to/logos-blockchain/target/release/logos-blockchain-node`)."
            )
        })?;
        let config_dir = env::var("LEZ_BEDROCK_CONFIG_DIR").map_err(|err| {
            anyhow::anyhow!(
                "LEZ_BEDROCK_CONFIG_DIR is required ({err}). Set it to the directory containing \
                 node-config.yaml and deployment-settings.yaml \
                 (see tools/e2e_bench/README.md for the expected layout)."
            )
        })?;
        let port: u16 = env::var("LEZ_BEDROCK_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(18080);

        let bin_path = PathBuf::from(&bin);
        if !bin_path.is_file() {
            bail!(
                "LEZ_BEDROCK_BIN does not point at a file: {bin}. Build it via \
                 `cargo build -p logos-blockchain-node --release` in logos-blockchain."
            );
        }
        let config_dir = PathBuf::from(config_dir);
        let node_config = config_dir.join("node-config.yaml");
        let dep_template = config_dir.join("deployment-settings.yaml");
        if !node_config.is_file() || !dep_template.is_file() {
            bail!(
                "LEZ_BEDROCK_CONFIG_DIR is missing node-config.yaml or \
                 deployment-settings.yaml at {}",
                config_dir.display()
            );
        }

        let workdir = tempfile::tempdir()
            .context("create bedrock workdir")?
            .keep();
        let dep_runtime = workdir.join("deployment-settings.yaml");
        let raw = std::fs::read_to_string(&dep_template).context("read deployment template")?;
        let timestamp = chrono_now_utc_string();
        let filled = raw.replace("PLACEHOLDER_CHAIN_START_TIME", &timestamp);
        std::fs::write(&dep_runtime, filled).context("write deployment-settings runtime")?;

        let log_path = workdir.join("bedrock.log");
        let log_file = std::fs::File::create(&log_path).context("create bedrock log")?;
        let log_err = log_file.try_clone().context("clone bedrock log")?;

        eprintln!(
            "BedrockHandle: launching {} (workdir {})",
            bin,
            workdir.display()
        );
        let child = Command::new(&bin_path)
            .current_dir(&workdir)
            .arg("--deployment")
            .arg(&dep_runtime)
            .arg(&node_config)
            .env("POL_PROOF_DEV_MODE", "true")
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err))
            .spawn()
            .context("spawn logos-blockchain-node")?;

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        wait_for_http(addr, Duration::from_secs(60))
            .await
            .context("bedrock HTTP did not come up in 60s")?;

        eprintln!("BedrockHandle: stdout/stderr at {}", log_path.display());
        Ok(Self {
            child: Some(child),
            addr,
            workdir,
        })
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for BedrockHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            eprintln!("BedrockHandle: stopping bedrock pid {}", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

async fn wait_for_http(addr: SocketAddr, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            // TCP accepts; give Bedrock a moment to finish chain bootstrap.
            tokio::time::sleep(Duration::from_secs(2)).await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("Bedrock at {addr} did not accept TCP within {timeout:?}");
}

fn chrono_now_utc_string() -> String {
    // Format: YYYY-MM-DD HH:MM:SS.000000 +00:00:00 (matches the deployment-settings template).
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.6f +00:00:00")
        .to_string()
}
