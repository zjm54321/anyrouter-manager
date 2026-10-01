mod app;
mod config;
mod diagnostics;
mod error;
mod gateway;
mod helper;
mod model;
mod store;
mod upstream;

#[cfg(test)]
mod tests;

use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), &'static str> {
    let mut args = std::env::args().skip(1);
    let path = match args.next().as_deref() {
        None => PathBuf::from("config.toml"),
        Some("--config") => PathBuf::from(args.next().ok_or("Usage: backend [--config PATH]")?),
        _ => return Err("Usage: backend [--config PATH]"),
    };
    if args.next().is_some() {
        return Err("Usage: backend [--config PATH]");
    }
    let config = config::Config::load(&path)?;
    let active = store::load(&config.state_path)?;
    let login = Arc::new(helper::BrowserLogin {
        directory: config.browser_helper_dir.clone(),
        timeout: Duration::from_secs(config.helper_timeout),
        diagnostics: config.login_diagnostics,
    });
    let upstream =
        upstream::Upstream::production().map_err(|_| "Cannot initialize upstream transport.")?;
    let bind = config.bind;
    let app = app::App::new(config, active, upstream, login);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| "Cannot bind local server.")?;
    println!("AnyRouter Manager listening at http://{bind}");
    axum::serve(listener, app::router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|_| "Local server failed.")
}
