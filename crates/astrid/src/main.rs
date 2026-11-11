//! astrid — the binary. TUI by default; `ask`, `eval`, `doctor` headless.

mod doctor;
mod eval;
mod eventlog;
mod session;

use anyhow::Result;
use clap::Parser;
use session::{HeadlessApproval, Session};

#[derive(Parser)]
#[command(
    name = "astrid",
    version,
    about = "A TUI-native agent operating system in Rust",
    arg_required_else_help = false
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// One-shot turn in the terminal (headless, no TUI).
    Ask {
        prompt: String,
        /// Auto-approve high/external tools (dev only).
        #[arg(long)]
        yes: bool,
    },
    /// Run the honest task harness.
    Eval {
        #[arg(long, default_value = "eval/tasks.json")]
        tasks: std::path::PathBuf,
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Live health report: config, memory, tools, skills, providers, hooks.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Tracing writes to stderr, which would corrupt the alternate-screen TUI.
    // Headless modes only; in TUI mode tracing macros stay no-op and receipt
    // events carry the story instead.
    if cli.command.is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_writer(std::io::stderr)
            .init();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        match cli.command {
            None => tui_main().await,
            Some(Command::Ask { prompt, yes }) => ask_main(&prompt, yes).await,
            Some(Command::Eval { tasks, out }) => {
                eval::run_eval(&tasks, out).await?;
                Ok(())
            }
            Some(Command::Doctor) => {
                let ok = doctor::run_doctor().await?;
                if !ok {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
    })
}

async fn tui_main() -> Result<()> {
    if !crossterm::tty::IsTty::is_tty(&std::io::stdin()) {
        anyhow::bail!(
            "astrid's TUI needs a terminal. For scripts use: astrid ask \"...\""
        );
    }
    let data_dir = std::env::var("ASTRID_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".astrid"));
    let (cfg, warnings) = astrid_core::AstridConfig::load(&data_dir)?;
    cfg.validate()?;
    for w in &warnings {
        eprintln!("warn: {w}");
    }

    let (session, rx) = Session::new(cfg, std::env::current_dir()?, None).await?;
    let session = std::sync::Arc::new(session);
    let bridge = session.approval_bridge.clone();

    // TUI channels
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(16);
    let cancel = session.cancel.clone();

    let runner_session = session.clone();
    let runner = tokio::spawn(async move {
        while let Some(input) = input_rx.recv().await {
            if input.trim() == "/quit" {
                break;
            }
            if let Err(e) = runner_session.turn(&input).await {
                tracing::error!("turn failed: {e}");
            }
        }
        runner_session.end().await;
    });

    let status = astrid_tui::TuiStatus {
        agent_name: session.cfg.agent_name.clone(),
        session_id: session.session_id.clone(),
        provider_desc: session.provider_desc.clone(),
        profile: session.cfg.permissions.as_str().to_string(),
        tool_names: session.registry.names(),
        skill_count: session.skills.list().len(),
        agent_count: session.roster.agents().len(),
        memory_pages: session.memory.page_count(),
        data_dir: data_dir.display().to_string(),
        hook_count: session.hooks.specs.len(),
        theme: session.cfg.tui.theme.clone(),
        ladder_summary: session.ladder.rung_summary(),
        mcp_lines: session.mcp_report.clone(),
        plugin_count: session.plugin_names.len(),
    };

    let res = astrid_tui::run_app(rx, input_tx, bridge, cancel, status).await;
    runner.abort();
    res.map_err(|e| anyhow::anyhow!("{e}"))
}

async fn ask_main(prompt: &str, yes: bool) -> Result<()> {
    let data_dir = std::env::var("ASTRID_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".astrid"));
    let (cfg, warnings) = astrid_core::AstridConfig::load(&data_dir)?;
    cfg.validate()?;
    for w in &warnings {
        eprintln!("warn: {w}");
    }
    let approval = if yes {
        HeadlessApproval::Approve
    } else {
        HeadlessApproval::Deny
    };
    let (session, mut rx) = Session::new(cfg, std::env::current_dir()?, Some(approval)).await?;

    // collect the transcript while the turn runs
    let collector = tokio::spawn(async move {
        let mut lines = Vec::new();
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let done = eventlog::is_turn_end(&ev);
                    lines.push(eventlog::render_event(&ev));
                    if done {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        lines
    });

    let report = session.turn(prompt).await?;
    session.end().await;
    let lines = match tokio::time::timeout(std::time::Duration::from_secs(5), collector).await {
        Ok(joined) => joined.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    for l in lines {
        println!("{l}");
    }
    eprintln!(
        "turn {} · outcome: {} · provider: {} · ladder_slid: {} · model_calls: {}",
        report.turn_id,
        eventlog::outcome_text(&report.outcome),
        report.provider,
        report.ladder_slid,
        report.model_calls,
    );
    Ok(())
}
