//! `victory`: the command-line front end.
//!
//! Every command reads the user's own files and writes only to stdout or to
//! the user's cache directory (outside the repository).

mod census;
mod inspect;
mod prebuild;
mod rtprobe;
mod run;
mod shader;
mod translate;

use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "victory",
    version,
    about = "Run an Xbox Series title natively on Windows"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise one PE image: headers, sections, imports, TLS, build id.
    Inspect {
        file: PathBuf,
        /// List every imported function.
        #[arg(long)]
        imports: bool,
        /// List every exported function.
        #[arg(long)]
        exports: bool,
    },
    /// Aggregate the imports of every PE image under the given paths and
    /// show which ones our runtime covers. Defaults to the configured
    /// game and test-title directories.
    Census {
        paths: Vec<PathBuf>,
        /// Do not count host pass-through as covered.
        #[arg(long)]
        strict: bool,
        /// List every missing `dll!function`, most used first.
        #[arg(long)]
        missing: bool,
    },
    /// List the `dll!export` pairs an import library describes.
    LibExports {
        lib: PathBuf,
        /// List every export, not just per-DLL counts.
        #[arg(long)]
        list: bool,
    },
    /// Find shader containers in files or directories (also inside engine
    /// archives, if uncompressed) and report their layout. Shader packs
    /// among the paths are listed last, their stages named from the
    /// containers found.
    ShaderInfo {
        paths: Vec<PathBuf>,
        /// Print one line per container found (and per pack pipeline).
        #[arg(long)]
        verbose: bool,
        /// Also list the decoded native instructions (implies --verbose).
        #[arg(long)]
        disasm: bool,
        /// Instead list the effects of shader databases: technique, pass,
        /// shaders (name, container hash, engine hash) and pass state.
        #[arg(long)]
        effects: bool,
    },
    /// Run every shader found (containers, SGD5 files) through the
    /// translator and report what it cannot handle yet.
    Translate {
        paths: Vec<PathBuf>,
        /// Check emitted DXIL with dxil.dll's validator (`VICTORY_DXIL`,
        /// else the newest Windows SDK's).
        #[arg(long)]
        validate: bool,
        /// Also create a host D3D12 pipeline for each emitted shader. With
        /// `--cache`: then make every pipeline of the title's shader pack
        /// into the host pipeline library `run` reads (on this GPU and
        /// driver; a second time only what is not there yet).
        #[arg(long)]
        pipelines: bool,
        /// Print one line per program and outcome.
        #[arg(long)]
        list: bool,
        /// Positive worker thread count (default: every logical core).
        #[arg(long)]
        jobs: Option<NonZeroUsize>,
        /// Only translate into the shader cache `run` reads (the warm-up):
        /// every container found, once, nothing else checked or listed
        /// (with `--pipelines`, then the shader pack's pipelines).
        #[arg(long, conflicts_with_all = ["validate", "list"])]
        cache: bool,
    },
    /// Prepare, map and bind a title executable; with --execute, run it.
    Run {
        exe: PathBuf,
        /// What unimplemented imports do when called.
        #[arg(long, value_enum, default_value_t = Unimplemented::Trap)]
        unimplemented: Unimplemented,
        /// Disable host pass-through (everything must be implemented by us).
        #[arg(long)]
        strict: bool,
        /// Actually start the title. Without it, stop after binding.
        #[arg(long)]
        execute: bool,
        /// The console model the title sees (device type, memory).
        #[arg(long, value_enum, default_value_t = Console::SeriesX)]
        console: Console,
        /// Arguments for the title, after `--` (it reads the whole command
        /// line; a title that relaunches itself passes some).
        #[arg(last = true)]
        title_args: Vec<String>,
    },
    /// Show the resolved configuration and check it.
    Config,
    /// Describe the host GPU and how it meets what the translation needs.
    /// Reads no title files; its output can go into a hardware report as
    /// it is (docs/gpu-compatibility.md).
    GpuInfo,
    /// Run the synthetic ray-tracing probe on the host GPU: generated
    /// scenes and rays through host ray-tracing structures and the three
    /// inline-query shaders, each path's hits compared with the CPU
    /// reference and the CPU model of the software traversal, each cost
    /// measured apart. Reads no title files (docs/dxr-validation.md); its
    /// times select no path and say nothing about a title. Runs only with
    /// VICTORY_RT_BACKEND=native, hybrid or auto set (hardware is the older
    /// name of native), and measures both paths under any of them.
    RtProbe {
        /// The seed the scenes and rays are generated from: a number, or
        /// hex after 0x. Without it, the seed of the reference's own
        /// documented tables.
        #[arg(long)]
        seed: Option<String>,
        /// Scene classes: `all`, or names with commas between.
        #[arg(long, default_value = "all")]
        classes: String,
        /// Ray records one timed dispatch lists: the class's rays, begun
        /// again as often as it takes.
        #[arg(long, default_value_t = victory_rt_probe::plan::DEFAULT_TIMED_RAYS)]
        rays: u32,
        /// Timed dispatches kept, per path and frame.
        #[arg(long, default_value_t = victory_rt_probe::plan::DEFAULT_REPEATS)]
        repeat: u32,
        /// Timed dispatches run before those and left out of the figures.
        #[arg(long, default_value_t = victory_rt_probe::plan::DEFAULT_DISCARD)]
        discard: u32,
        /// Loop mode: this many iterations, per class and path, of what a
        /// path adds per frame (0: none).
        #[arg(long = "loop", default_value_t = 0)]
        loops: u32,
        /// Turn the Direct3D 12 debug layer on and print what it says. For
        /// finding out why a call is refused: no time of such a run is a
        /// measurement, and its file says so.
        #[arg(long)]
        debug_layer: bool,
        /// The result table. The hit records of each path and the list of
        /// mismatches are written beside it, named after it. Outside the
        /// repository.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Unimplemented {
    Trap,
    Zero,
}

#[derive(Clone, Copy, ValueEnum)]
enum Console {
    SeriesX,
    SeriesS,
}

/// Diagnostics: the heap bytes Rust code holds, for the statistics.
#[cfg(all(windows, feature = "diagnostics"))]
#[global_allocator]
static ALLOC: victory_gpu::CountingAlloc = victory_gpu::CountingAlloc;

fn main() -> Result<()> {
    victory_core::logging::init("info");
    // The audio recorder `run` starts for `VICTORY_AUDIO_CAPTURE`.
    #[cfg(all(windows, feature = "diagnostics"))]
    if victory_hle::modules::audio::capture::child() {
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            file,
            imports,
            exports,
        } => inspect::run(&file, imports, exports),
        Command::Census {
            paths,
            strict,
            missing,
        } => census::run(paths, strict, missing),
        Command::LibExports { lib, list } => inspect::lib_exports(&lib, list),
        Command::ShaderInfo {
            paths,
            verbose,
            disasm,
            effects,
        } => shader::run(paths, verbose || disasm, disasm, effects),
        Command::Translate {
            paths,
            jobs,
            pipelines,
            cache: true,
            ..
        } => translate::warm(paths, jobs.map(NonZeroUsize::get), pipelines),
        Command::Translate {
            paths,
            validate,
            pipelines,
            list,
            jobs,
            cache: false,
        } => translate::run(
            paths,
            validate,
            pipelines,
            list,
            jobs.map(NonZeroUsize::get),
        ),
        Command::Run {
            exe,
            unimplemented,
            strict,
            execute,
            console,
            title_args: _,
        } => {
            let code = run::run(&exe, unimplemented, strict, execute, console)?;
            if code != 0 {
                // Preserve all 32 bits of the Windows process status.
                std::process::exit(code as i32);
            }
            Ok(())
        }
        Command::Config => show_config(),
        Command::GpuInfo => gpu_info(),
        Command::RtProbe {
            seed,
            classes,
            rays,
            repeat,
            discard,
            loops,
            debug_layer,
            out,
        } => rtprobe::run(rtprobe::Args {
            seed,
            classes,
            rays,
            repeat,
            discard,
            loops,
            debug_layer,
            out,
        }),
    }
}

fn gpu_info() -> Result<()> {
    let caps = victory_gpu::host_caps(true)
        .ok_or_else(|| anyhow::anyhow!("no host Direct3D 12 device could be created"))?;
    print!("{}", victory_gpu::caps::report(&caps));
    Ok(())
}

fn show_config() -> Result<()> {
    let cfg = victory_core::Config::load()?;
    let status = |p: &Option<PathBuf>| match p {
        Some(p) if p.exists() => format!("{}", p.display()),
        Some(p) => format!("{}  (does not exist)", p.display()),
        None => "(not set)".to_owned(),
    };
    println!(
        "config file : {}",
        cfg.source
            .as_ref()
            .map_or("(none)".into(), |p| p.display().to_string())
    );
    println!("game_dir    : {}", status(&cfg.game_dir));
    println!("cache_dir   : {}", cfg.cache_dir.display());
    println!("data_dir    : {}", cfg.data_dir.display());
    println!("fidelityfx  : {}", status(&cfg.fidelityfx_dir));
    for t in &cfg.test_titles {
        println!("test_title  : {}", status(&Some(t.clone())));
    }
    check_policy(&cfg)
}

/// Reports each configured path the asset policy refuses, and fails if
/// there is one, so that `victory config` exits nonzero.
fn check_policy(cfg: &victory_core::Config) -> Result<()> {
    let mut violations = 0;
    for p in cfg
        .game_dir
        .iter()
        .chain(&cfg.test_titles)
        .chain([&cfg.cache_dir, &cfg.data_dir])
    {
        if let Err(e) = victory_core::ensure_outside_repo(p) {
            println!("POLICY VIOLATION: {e}");
            violations += 1;
        }
    }
    anyhow::ensure!(
        violations == 0,
        "{violations} configured path(s) violate the asset policy"
    );
    Ok(())
}

/// Paths given on the command line, or the configured game + test titles.
pub(crate) fn default_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if !paths.is_empty() {
        return Ok(paths);
    }
    let cfg = victory_core::Config::load()?;
    let all: Vec<PathBuf> = cfg.game_dir.into_iter().chain(cfg.test_titles).collect();
    anyhow::ensure!(
        !all.is_empty(),
        "no paths given and none configured (see victory.example.toml)"
    );
    Ok(all)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::check_policy;

    fn outside() -> PathBuf {
        std::env::temp_dir().join("victory-config-test")
    }

    #[test]
    fn config_check_passes_paths_outside_the_repository() {
        let cfg = victory_core::Config {
            cache_dir: outside().join("cache"),
            data_dir: outside().join("data"),
            ..Default::default()
        };
        assert!(check_policy(&cfg).is_ok());
    }

    #[test]
    fn config_check_fails_a_path_inside_the_repository() {
        let cfg = victory_core::Config {
            game_dir: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("game")),
            cache_dir: outside().join("cache"),
            data_dir: outside().join("data"),
            ..Default::default()
        };
        let err = check_policy(&cfg).unwrap_err();
        assert!(err.to_string().contains("1 configured path"), "{err}");
    }
}
