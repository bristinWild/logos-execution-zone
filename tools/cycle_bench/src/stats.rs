//! Small helper for best / mean / stdev over wall-time samples.
//!
//! We report both best-of-N (the figure that strips OS noise and matches what most
//! bench READMEs print) and mean +/- stdev (the figure the fee model wants, since
//! it cares about the steady-state cost not a single fastest sample).

use serde::Serialize;

#[derive(Debug, Serialize, Clone, Copy, Default)]
pub struct Stats {
    pub n: usize,
    pub best_ms: f64,
    pub mean_ms: f64,
    pub stdev_ms: f64,
}

impl Stats {
    pub fn from_samples(samples: &[f64]) -> Self {
        let n = samples.len();
        if n == 0 {
            return Self::default();
        }
        let best_ms = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let sum: f64 = samples.iter().sum();
        let mean_ms = sum / n as f64;
        let stdev_ms = if n > 1 {
            let var: f64 = samples
                .iter()
                .map(|s| {
                    let d = s - mean_ms;
                    d * d
                })
                .sum::<f64>()
                / (n - 1) as f64;
            var.sqrt()
        } else {
            0.0
        };
        Self {
            n,
            best_ms,
            mean_ms,
            stdev_ms,
        }
    }

    /// Format as `best / mean ± stdev (n=N)` for table display.
    pub fn format(&self) -> String {
        format!(
            "{:.2} / {:.2} ± {:.2} (n={})",
            self.best_ms, self.mean_ms, self.stdev_ms, self.n,
        )
    }
}
