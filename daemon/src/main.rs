//! parakeetd: local speech-to-text daemon for fcitx5-parakeet.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{info, LevelFilter};
use parakeetd::{config, server, sherpa};

struct Args {
    config: Option<PathBuf>,
    socket: Option<PathBuf>,
    verbose: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config: None,
        socket: None,
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                args.config = Some(
                    it.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| anyhow::anyhow!("--config needs a path"))?,
                )
            }
            "--socket" => {
                args.socket = Some(
                    it.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| anyhow::anyhow!("--socket needs a path"))?,
                )
            }
            "-v" | "--verbose" => args.verbose = true,
            "-h" | "--help" => {
                println!("usage: parakeetd [--config FILE] [--socket PATH] [-v]");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(args)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if args.verbose { "debug" } else { "info" }),
    )
    .filter_module("tokio", LevelFilter::Info)
    .format_timestamp_millis()
    .init();

    let mut cfg = config::load(args.config.as_deref())?;
    if let Some(socket) = args.socket {
        cfg.socket_path = socket;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let (listener, owns_path) = server::listen(&cfg.socket_path)?;
        let socket_path = cfg.socket_path.clone();
        let languages = cfg.languages().join(", ");
        let daemon = server::Daemon::new(cfg);
        info!(
            "parakeetd {} ready (sherpa-onnx {}, languages: {languages})",
            env!("CARGO_PKG_VERSION"),
            sherpa::version()
        );
        let preload = {
            let daemon = Arc::clone(&daemon);
            tokio::spawn(async move { daemon.preload().await })
        };
        let serve = tokio::spawn(Arc::clone(&daemon).serve(listener));

        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        info!("shutting down");
        serve.abort();
        preload.abort();
        if owns_path {
            let _ = std::fs::remove_file(&socket_path);
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}
