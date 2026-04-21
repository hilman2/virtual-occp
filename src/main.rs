use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use virtual_occp::cli;
use virtual_occp::manager::{Manager, StationDef};
use virtual_occp::manager_web;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();
    args.validate()?;

    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,virtual_occp=debug")),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let data_dir = std::path::PathBuf::from(&args.data_dir);
    std::fs::create_dir_all(&data_dir)?;

    let mgr = Manager::new(data_dir.clone());
    mgr.load_persisted().await?;

    for cfg in cli::parse_stations(&args.station)? {
        let def = StationDef {
            id: cfg.id,
            http_port: cfg.http_port,
            version: cfg.version,
            csms_url: cfg.csms_url,
            autostart: true,
            username: cfg.username,
            password: cfg.password,
        };
        if let Err(e) = mgr.upsert(def).await {
            tracing::warn!("Could not add CLI station: {e}");
        }
    }

    mgr.autostart_all().await?;

    let mgr_task = if let Some(port) = args.manager_port {
        let m = mgr.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = manager_web::serve(m, port).await {
                tracing::error!("Manager web server stopped: {e:?}");
            }
        }))
    } else {
        None
    };

    tracing::info!("virtual-occp is running. Press Ctrl+C to quit.");
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Shutting down...");
    if let Some(t) = mgr_task {
        t.abort();
    }
    Ok(())
}
