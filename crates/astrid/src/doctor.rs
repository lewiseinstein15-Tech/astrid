//! `astrid doctor`: live health of config, memory, tools, skills, workforce,
//! providers, hooks. Problems are printed with reasons and exit non-zero.

use anyhow::Result;

pub async fn run_doctor() -> Result<bool> {
    let data_dir = std::env::var("ASTRID_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".astrid"));
    let mut problems = Vec::new();

    println!("astrid doctor — data dir: {}", data_dir.display());

    // config
    match astrid_core::AstridConfig::load(&data_dir) {
        Ok((cfg, warnings)) => {
            for w in &warnings {
                println!("  warn   config: {w}");
            }
            if let Err(e) = cfg.validate() {
                problems.push(format!("config invalid: {e}"));
            } else {
                println!("  ok     config (profile {})", cfg.permissions.as_str());
            }

            // memory
            match astrid_memory::MemoryKernel::open(&data_dir.join("memory")) {
                Ok(mem) => {
                    match mem.verify() {
                        Ok(drift) if drift.is_empty() => {
                            println!("  ok     memory ({} pages)", mem.page_count())
                        }
                        Ok(drift) => {
                            for d in &drift {
                                println!("  warn   memory: {d}");
                            }
                        }
                        Err(e) => problems.push(format!("memory verify: {e}")),
                    }
                }
                Err(e) => problems.push(format!("memory open: {e}")),
            }

            // tools
            let registry = astrid_tools::ToolRegistry::with_builtins();
            println!("  ok     tools ({} registered)", registry.names().len());

            // skills + workforce
            match astrid_workforce::SkillIndex::load(Some(&data_dir.join("skills"))) {
                Ok(s) => println!("  ok     skills ({} indexed)", s.list().len()),
                Err(e) => problems.push(format!("skills: {e}")),
            }
            let roster = astrid_workforce::Roster::bundled();
            println!(
                "  ok     workforce ({} agents, {} divisions)",
                roster.agents().len(),
                roster.divisions().len()
            );

            // providers
            let ladder = astrid_providers::ProviderLadder::from_config(&cfg.providers);
            for rung in ladder.rung_summary() {
                println!("  info   provider {rung}");
            }

            // hooks
            match astrid_core::HookRegistry::load(&data_dir) {
                Ok(h) if h.is_empty() => println!("  info   hooks (none registered)"),
                Ok(h) => println!("  ok     hooks ({} registered)", h.specs.len()),
                Err(e) => problems.push(format!("hooks: {e}")),
            }
        }
        Err(e) => problems.push(format!("config load: {e}")),
    }

    if problems.is_empty() {
        println!("\nall checks passed");
        Ok(true)
    } else {
        println!("\n{} problem(s):", problems.len());
        for p in &problems {
            println!("  - {p}");
        }
        Ok(false)
    }
}
