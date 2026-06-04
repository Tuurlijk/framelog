mod analyze;
mod capture;
mod collector;
mod config;
mod context;
mod error;
mod hardware;
mod journal;
mod model;
mod service;
mod store;
mod summary;
mod system_info;
mod throttle;
mod util;
mod version;
mod web;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::analyze::{self as evidence, validate_export_bundle};
use crate::collector::{make_collector, CollectorKind};
use crate::config::Config;
use crate::error::Result;
use crate::model::{AnalysisOptions, ExportBundle};
use crate::service::CollectorService;
use crate::store::Store;

#[derive(Parser, Debug)]
#[command(
    name = "framelog",
    about = "Log AMDGPU throttle flags and correlate with systemd journal",
    version = crate::version::VERSION_INFO
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, env = "FRAMELOG_DB")]
    db: Option<PathBuf>,

    #[arg(long, global = true, default_value = "1000")]
    interval_ms: u64,

    #[arg(long, global = true, default_value = "127.0.0.1:8787")]
    bind: String,

    #[arg(long, global = true, default_value_t = 60)]
    journal_before_secs: i64,

    #[arg(long, global = true, default_value_t = 30)]
    journal_after_secs: i64,

    #[arg(long, global = true)]
    apu_only: bool,

    #[arg(long, global = true, value_enum, default_value_t = CollectorKind::LibAmdgpu)]
    collector: CollectorKind,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Sample metrics and persist to SQLite
    Collect,
    /// Serve the local graph UI and API
    Serve,
    /// Run collector and web server (for systemd)
    Run,
    /// Print one live sample to stdout
    Inspect,
    /// Report detected hardware, collector fit, and available signals
    Capabilities,
    /// Write a timestamped JSON export bundle for the stored data
    Dump {
        /// Exact output file path. Defaults to ./framelog-dump-<from>-<to>-<exported>.json
        #[arg(long)]
        output: Option<PathBuf>,
        /// Directory for the generated dump file
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Start timestamp in Unix milliseconds. Defaults to earliest stored telemetry.
        #[arg(long = "from-ms")]
        from_ms: Option<i64>,
        /// End timestamp in Unix milliseconds. Defaults to latest stored telemetry.
        #[arg(long = "to-ms")]
        to_ms: Option<i64>,
        /// Limit dump to one sampled GPU PCI address.
        #[arg(long)]
        device_pci: Option<String>,
    },
    /// Guided evidence capture: backup, reset, collect, export, and report
    Capture {
        /// How long to collect (e.g. 20m, 1h, 300s)
        #[arg(short, long)]
        duration: String,
        /// Short label for the output folder and report (e.g. framework-146)
        #[arg(short, long, default_value = "capture")]
        issue: String,
        /// Output directory (default: ./framelog-capture-<issue>-<timestamp>)
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Do not write pre_capture_backup.json before reset
        #[arg(long)]
        skip_pre_backup: bool,
        /// Limit export/report to one GPU PCI address
        #[arg(long)]
        device_pci: Option<String>,
    },
    /// Analyze an export bundle (deterministic findings)
    Analyze {
        /// Path to export.json (or any framelog_export JSON file)
        input: PathBuf,
        /// Write output to this file instead of stdout
        #[arg(long)]
        output: Option<PathBuf>,
        /// Output format: markdown (default) or json
        #[arg(long, value_enum, default_value_t = AnalyzeFormat::Markdown)]
        format: AnalyzeFormat,
        /// Also print an AI handoff prompt to stderr (markdown/json go to stdout/file)
        #[arg(long)]
        ai_prompt: bool,
        /// Override analysis window start (Unix ms)
        #[arg(long = "from-ms")]
        from_ms: Option<i64>,
        /// Override analysis window end (Unix ms)
        #[arg(long = "to-ms")]
        to_ms: Option<i64>,
        /// Limit analysis to one GPU PCI address
        #[arg(long)]
        device_pci: Option<String>,
    },
    /// Clear collected telemetry after optionally writing a backup dump
    Reset {
        /// Confirm destructive reset.
        #[arg(long)]
        yes: bool,
        /// Clear data without writing a backup export first.
        #[arg(long)]
        no_backup: bool,
        /// Exact backup file path. Defaults next to the SQLite database.
        #[arg(long)]
        backup: Option<PathBuf>,
        /// Directory for the generated backup file.
        #[arg(long)]
        backup_dir: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum AnalyzeFormat {
    Markdown,
    Json,
}

struct DumpOptions<'a> {
    output: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    default_dir: &'a Path,
    prefix: &'a str,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    device_pci: Option<&'a str>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("framelog=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();
    let mut config = Config::default();
    if let Some(db) = cli.db {
        config.db_path = db;
    }
    config.interval_ms = cli.interval_ms;
    config.bind = cli
        .bind
        .parse::<SocketAddr>()
        .map_err(|e| error::Error::Other(format!("invalid bind address: {e}")))?;
    config.journal_before_secs = cli.journal_before_secs;
    config.journal_after_secs = cli.journal_after_secs;
    config.apu_only = cli.apu_only;
    config.fake = matches!(cli.collector, CollectorKind::Fake);

    match cli.command {
        Commands::Collect => run_collect(config, cli.collector).await,
        Commands::Serve => run_serve(config, cli.collector, cli.apu_only).await,
        Commands::Run => run_both(config, cli.collector).await,
        Commands::Inspect => run_inspect(cli.collector, config.apu_only).await,
        Commands::Capabilities => run_capabilities(cli.collector, config.apu_only).await,
        Commands::Capture {
            duration,
            issue,
            output_dir,
            skip_pre_backup,
            device_pci,
        } => {
            run_capture(
                config,
                cli.collector,
                duration,
                issue,
                output_dir,
                skip_pre_backup,
                device_pci,
            )
            .await
        }
        Commands::Dump {
            output,
            output_dir,
            from_ms,
            to_ms,
            device_pci,
        } => run_dump(config, output, output_dir, from_ms, to_ms, device_pci).await,
        Commands::Analyze {
            input,
            output,
            format,
            ai_prompt,
            from_ms,
            to_ms,
            device_pci,
        } => run_analyze(input, output, format, ai_prompt, from_ms, to_ms, device_pci).await,
        Commands::Reset {
            yes,
            no_backup,
            backup,
            backup_dir,
        } => run_reset(config, yes, no_backup, backup, backup_dir).await,
    }
}

async fn run_analyze(
    input: PathBuf,
    output: Option<PathBuf>,
    format: AnalyzeFormat,
    ai_prompt: bool,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    device_pci: Option<String>,
) -> Result<()> {
    let bytes = tokio::fs::read(&input).await?;
    let bundle: ExportBundle = serde_json::from_slice(&bytes)?;
    validate_export_bundle(&bundle).map_err(error::Error::BadRequest)?;

    let opts = AnalysisOptions {
        from_ms,
        to_ms,
        device_pci,
        issue_tag: None,
    };
    let report = evidence::analyze_bundle(&bundle, &opts);

    let body = match format {
        AnalyzeFormat::Json => serde_json::to_string_pretty(&report)?,
        AnalyzeFormat::Markdown => evidence::render_markdown(&report),
    };

    if let Some(path) = output {
        tokio::fs::write(&path, &body).await?;
        println!("wrote {}", path.display());
    } else {
        print!("{body}");
    }

    if ai_prompt {
        eprintln!(
            "\n--- AI handoff prompt ---\n{}",
            evidence::render_ai_prompt(&report)
        );
    }

    Ok(())
}

async fn open_store(config: &Config) -> Result<Arc<Store>> {
    Ok(Arc::new(Store::open(&config.db_path).await?))
}

async fn run_collect(config: Config, kind: CollectorKind) -> Result<()> {
    let store = open_store(&config).await?;
    let collector = make_collector(kind, config.apu_only);
    CollectorService::new(store, collector, config).run().await
}

async fn run_serve(config: Config, kind: CollectorKind, apu_only: bool) -> Result<()> {
    let store = open_store(&config).await?;
    web::serve(config.bind, store, kind, apu_only).await
}

async fn run_both(config: Config, kind: CollectorKind) -> Result<()> {
    let store = open_store(&config).await?;
    let collector = make_collector(kind, config.apu_only);

    let collector_store = Arc::clone(&store);
    let collector_config = config.clone();
    let collector_handle = tokio::spawn(async move {
        CollectorService::new(collector_store, collector, collector_config)
            .run()
            .await
    });

    let bind = config.bind;
    let apu_only = config.apu_only;
    let web_handle = tokio::spawn(async move { web::serve(bind, store, kind, apu_only).await });

    tokio::select! {
        res = collector_handle => match res {
            Ok(Err(e)) => Err(e),
            Ok(Ok(())) => Ok(()),
            Err(e) => Err(error::Error::Other(format!("collector task: {e}"))),
        },
        res = web_handle => match res {
            Ok(Err(e)) => Err(e),
            Ok(Ok(())) => Ok(()),
            Err(e) => Err(error::Error::Other(format!("web task: {e}"))),
        },
    }
}

async fn run_capture(
    config: Config,
    kind: CollectorKind,
    duration: String,
    issue: String,
    output_dir: Option<PathBuf>,
    skip_pre_backup: bool,
    device_pci: Option<String>,
) -> Result<()> {
    let duration = capture::parse_duration(&duration)?;
    capture::run_capture(
        config,
        kind,
        capture::CaptureOptions {
            duration,
            issue,
            output_dir,
            device_pci,
            skip_pre_backup,
        },
    )
    .await?;
    Ok(())
}

async fn run_capabilities(kind: CollectorKind, apu_only: bool) -> Result<()> {
    let report = crate::hardware::capabilities::probe(kind, apu_only);
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn run_inspect(kind: CollectorKind, apu_only: bool) -> Result<()> {
    let capabilities = crate::hardware::capabilities::probe(kind, apu_only);
    let collector = make_collector(kind, apu_only);
    let mut out = serde_json::json!({ "capabilities": capabilities });
    match collector.sample() {
        Ok(samples) => {
            out["samples"] = serde_json::to_value(samples)?;
        }
        Err(err) => {
            out["sample_error"] = serde_json::Value::String(err.to_string());
        }
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

async fn run_dump(
    config: Config,
    output: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    device_pci: Option<String>,
) -> Result<()> {
    let store = open_store(&config).await?;
    let default_dir = std::env::current_dir()?;
    let path = write_dump(
        &store,
        DumpOptions {
            output,
            output_dir,
            default_dir: &default_dir,
            prefix: "framelog-dump",
            from_ms,
            to_ms,
            device_pci: device_pci.as_deref(),
        },
    )
    .await?;
    println!("wrote {}", path.display());
    Ok(())
}

async fn run_reset(
    config: Config,
    yes: bool,
    no_backup: bool,
    backup: Option<PathBuf>,
    backup_dir: Option<PathBuf>,
) -> Result<()> {
    if !yes {
        return Err(error::Error::BadRequest(
            "reset clears collected telemetry; rerun with --yes to confirm".into(),
        ));
    }
    if no_backup && (backup.is_some() || backup_dir.is_some()) {
        return Err(error::Error::BadRequest(
            "--no-backup cannot be combined with --backup or --backup-dir".into(),
        ));
    }

    let store = open_store(&config).await?;
    if !no_backup {
        let default_dir = config
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        match write_dump(
            &store,
            DumpOptions {
                output: backup,
                output_dir: backup_dir,
                default_dir: &default_dir,
                prefix: "framelog-backup",
                from_ms: None,
                to_ms: None,
                device_pci: None,
            },
        )
        .await
        {
            Ok(path) => println!("backup written to {}", path.display()),
            Err(error::Error::BadRequest(message)) if message == "no telemetry data to dump" => {
                println!("no telemetry data to back up");
            }
            Err(err) => return Err(err),
        }
    }

    store.assert_not_collecting(5_000).await?;
    store.reset_data_collection().await?;
    println!("reset collected telemetry in {}", config.db_path.display());
    println!(
        "if the framelog service is still running, it may start writing fresh rows immediately"
    );
    Ok(())
}

async fn write_dump(store: &Store, opts: DumpOptions<'_>) -> Result<PathBuf> {
    if opts.output.is_some() && opts.output_dir.is_some() {
        return Err(error::Error::BadRequest(
            "--output cannot be combined with --output-dir/--backup-dir".into(),
        ));
    }

    let (from_ms, to_ms) = dump_range(store, opts.from_ms, opts.to_ms).await?;
    let mut bundle = store
        .export_bundle_unbounded(from_ms, to_ms, opts.device_pci)
        .await?;
    evidence::attach_analysis(
        &mut bundle,
        &AnalysisOptions {
            from_ms: Some(from_ms),
            to_ms: Some(to_ms),
            device_pci: opts.device_pci.map(str::to_string),
            issue_tag: None,
        },
    );
    let path = dump_path(
        opts.output,
        opts.output_dir,
        opts.default_dir,
        opts.prefix,
        &bundle,
    )?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

async fn dump_range(store: &Store, from_ms: Option<i64>, to_ms: Option<i64>) -> Result<(i64, i64)> {
    let bounds = store.time_bounds().await?;
    let Some(from_ms) = from_ms.or(bounds.min_ts_ms) else {
        return Err(error::Error::BadRequest("no telemetry data to dump".into()));
    };
    let Some(to_ms) = to_ms.or(bounds.max_ts_ms) else {
        return Err(error::Error::BadRequest("no telemetry data to dump".into()));
    };
    if from_ms > to_ms {
        return Err(error::Error::BadRequest("from-ms must be <= to-ms".into()));
    }
    Ok((from_ms, to_ms))
}

fn dump_path(
    output: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    default_dir: &Path,
    prefix: &str,
    bundle: &ExportBundle,
) -> Result<PathBuf> {
    if let Some(path) = output {
        return Ok(path);
    }
    let dir = output_dir.unwrap_or_else(|| default_dir.to_path_buf());
    Ok(dir.join(format!(
        "{prefix}-{}-{}-{}.json",
        bundle.from_ms, bundle.to_ms, bundle.exported_at_ms
    )))
}
