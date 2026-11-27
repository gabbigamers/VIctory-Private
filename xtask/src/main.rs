//! Repository maintenance. Run with `cargo xtask <command>`.

mod perf;
mod policy;
mod rt_summary;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enforce the asset policy: the repository holds only small text files
    /// we wrote. Checks tracked and untracked-but-not-ignored files.
    Policy {
        /// Check only what is staged for commit (used by the pre-commit hook).
        #[arg(long)]
        staged: bool,
    },
    /// Point git at .githooks/ so every commit runs the policy check.
    InstallHooks,
    /// Summarise a run's `VICTORY_PERF_LOG` as the figures docs/progress.md
    /// records: frame times over the world's periods, and memory as working
    /// set, commit, paging file and video memory, each with its peak.
    PerfSummary {
        /// The run's log.
        log: PathBuf,
        /// An earlier run's log to compare with: each figure before and
        /// after, and whether the change is better or worse.
        #[arg(long)]
        against: Option<PathBuf>,
    },
    /// Summarise a result table of `victory rt-probe` (docs/dxr-validation.md):
    /// correctness counts, the card's time of traversal, of builds and of
    /// refits, processor time, waits and memory, each apart. The probe's
    /// times select no path and say nothing about a title.
    RtSummary {
        /// The run's result table (`rt-probe.csv`).
        file: PathBuf,
        /// An earlier run of the same seed to compare with: the counts of
        /// every row and the hit records beside the two tables must be
        /// equal; times are shown side by side.
        #[arg(long)]
        against: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let root = workspace_root();
    match Cli::parse().command {
        Cmd::Policy { staged } => policy::run(&root, staged),
        Cmd::PerfSummary { log, against } => perf::run(&log, against.as_deref()),
        Cmd::RtSummary { file, against } => rt_summary::run(&file, against.as_deref()),
        Cmd::InstallHooks => {
            let status = Command::new("git")
                .args(["config", "core.hooksPath", ".githooks"])
                .current_dir(&root)
                .status()
                .context("running git")?;
            anyhow::ensure!(
                status.success(),
                "git config failed (is this a git repository?)"
            );
            println!("hooks installed: commits now run `cargo xtask policy --staged`");
            Ok(())
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .into()
}
