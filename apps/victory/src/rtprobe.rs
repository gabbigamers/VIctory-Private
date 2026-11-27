//! `victory rt-probe`: the synthetic ray-tracing probe
//! (docs/dxr-validation.md). Generated scenes and rays go through host
//! ray-tracing structures and the three inline-query shaders on this
//! machine's GPU; each path's hit records are compared with the CPU
//! reference and with the CPU model of the software traversal; each cost
//! is measured apart. It reads no file of any title and writes text files
//! of measurements and synthetic data, outside the repository.
//!
//! Everything it decides is `victory_rt_probe`'s and tested there on any
//! host; this file reads the options, tries the places the files go to,
//! opens the device and writes the files. **It has not run on any device.**
//!
//! It is behind the switch a run's hardware ray tracing is behind: without
//! `VICTORY_RT_BACKEND=hardware` (or `auto`) it opens no device and runs
//! nothing.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use victory_rt_probe::device::{Device, asked_by_environment};
use victory_rt_probe::hardware::Hardware;
use victory_rt_probe::{adapter, file, plan, run};

/// The options as typed.
pub struct Args {
    pub seed: Option<String>,
    pub classes: String,
    pub rays: u32,
    pub repeat: u32,
    pub discard: u32,
    pub loops: u32,
    pub debug_layer: bool,
    pub out: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let options = plan::Options {
        seed: match &args.seed {
            Some(text) => plan::seed(text).map_err(|e| anyhow!(e))?,
            None => adapter::default_seed(),
        },
        classes: plan::classes(&args.classes).map_err(|e| anyhow!(e))?,
        timed_rays: args.rays,
        repeats: args.repeat,
        discard: args.discard,
        loops: args.loops,
    };
    options.check().map_err(|e| anyhow!(e))?;
    // The files are measurements and synthetic data, but the repository
    // holds only what its policy lists: they go outside it.
    victory_core::ensure_outside_repo(&args.out)
        .with_context(|| format!("--out {}: the result files go outside the repository", args.out.display()))?;
    let outputs = file::Outputs::beside(&args.out);

    println!("rt-probe: synthetic scenes and rays only; no file of any title is read");
    let not_run = |why: &dyn std::fmt::Display| {
        println!("rt-probe: NOT RUN: {why}");
        println!(
            "rt-probe: nothing was dispatched and no result file was written; a file already at {} is an earlier \
             run's",
            args.out.display()
        );
    };
    // Behind the switch, before anything is made: not a folder either.
    if let Err(off) = asked_by_environment() {
        not_run(&off);
        bail!("the probe did not run: the hardware ray-tracing backend is not asked for");
    }
    // A run is long and writes its files when it is done: whether they can
    // be written is found out now. Nothing an earlier run left is changed.
    if let Err(why) = outputs.prepare() {
        not_run(&format!("the result files cannot be written: {why}"));
        bail!("the probe did not run: its result files cannot be written");
    }
    let mut device = match Hardware::open(args.debug_layer) {
        Ok(device) => device,
        Err(unavailable) => {
            not_run(&unavailable);
            bail!("the probe did not run on this device");
        }
    };
    for (key, value) in device.identity() {
        println!("rt-probe: device: {key} = {value}");
    }
    if args.debug_layer {
        println!(
            "rt-probe: the Direct3D 12 debug layer is on: its messages follow as `validation layer` lines, and no \
             time of this run is a measurement"
        );
    }
    let results =
        run::run(&mut device, &options, &mut |line| println!("rt-probe: {line}")).map_err(|e| anyhow!(e))?;

    let written = file::write(&results, &outputs);
    for (path, bytes) in &written.files {
        println!("rt-probe: wrote {} ({bytes} bytes)", path.display());
    }
    for line in &written.failed {
        println!("rt-probe: NOT WRITTEN: {line}");
    }
    if let Some(table) = &written.table_lost {
        // The run is done and its table has no file: the console is the
        // one place left for it.
        println!("rt-probe: the table could not be written; it follows, between the two lines of dashes");
        println!("--------\n{table}--------");
    }
    println!("rt-probe: rows: {}", results.table.value("rows").unwrap_or("not counted"));
    println!("rt-probe: defects listed for replay: {}", results.mismatches.len());
    println!(
        "rt-probe: the verdicts are victory-rt's, under that crate's policy and not yet the rule of \
         docs/dxr-compatibility.md; the times are of synthetic scenes, select no path and say nothing about a title"
    );
    if !written.failed.is_empty() {
        bail!("{} of the run's files could not be written", written.failed.len());
    }
    println!("rt-probe: to read it: cargo xtask rt-summary {}", args.out.display());
    Ok(())
}
