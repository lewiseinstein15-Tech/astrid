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
            println!("  ok     tools ({} builtin registered)", registry.names().len());

            // plugins
            let (plugins, plugin_problems) = astrid_tools::PluginTool::scan(&data_dir);
            if plugin_problems.is_empty() {
                println!("  ok     plugins ({} loaded)", plugins.len());
            } else {
                for p in &plugin_problems {
                    println!("  warn   plugins: {p}");
                }
            }

            // mcp servers: real handshake against each configured server
            if !cfg.mcp.enabled || cfg.mcp.servers.is_empty() {
                println!("  info   mcp (no servers configured)");
            } else {
                for s in &cfg.mcp.servers {
                    if !s.enabled {
                        println!("  info   mcp {}: disabled", s.name);
                        continue;
                    }
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(cfg.mcp.timeout_ms.min(5_000)),
                        astrid_mcp::connect_and_adapt(s, cfg.mcp.timeout_ms),
                    )
                    .await
                    {
                        Ok(Ok((_, adapters))) => {
                            println!("  ok     mcp {} ({} tools)", s.name, adapters.len())
                        }
                        Ok(Err(e)) => problems.push(format!("mcp {}: {e}", s.name)),
                        Err(_) => problems.push(format!("mcp {}: connect timed out", s.name)),
                    }
                }
            }

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
