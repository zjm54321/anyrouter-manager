mod app;
mod checkin;
mod config;
mod diagnostics;
mod error;
mod gateway;
mod helper;
mod hybrid;
mod model;
mod model_filter;
mod request_log;
mod store;
mod supervisor;
mod upstream;

#[cfg(test)]
mod tests;

use std::{path::PathBuf, sync::Arc, time::Duration};

fn main() {
    supervisor::dispatch();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime initialization");
    if let Err(message) = runtime.block_on(run()) {
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
    let browser = Arc::new(helper::BrowserLogin {
        directory: config.browser_helper_dir.clone(),
        timeout: Duration::from_secs(config.helper_timeout),
        diagnostics: config.login_diagnostics,
        executable: config.browser_helper_executable.clone(),
        container_mode: config.container_mode,
    });
    let login = Arc::new(hybrid::HybridLoginProvider::new(
        browser,
        Duration::from_secs(config.helper_timeout),
    ));
    let upstream =
        upstream::Upstream::production().map_err(|_| "Cannot initialize upstream transport.")?;
    let bind = config.bind;
    let logs = request_log::LogSink::open(config.request_log_path())
        .await
        .map_err(|_| "Cannot initialize private request log.")?;
    logs.flush()
        .await
        .map_err(|_| "Cannot initialize private request log.")?;
    let app = app::App::new_with_logs(config, active, upstream, login, Some(logs));
    *app.checkin.lock().await = checkin::load(&app.config.checkin_path())?;
    {
        let accounts = app.accounts.lock().await;
        let checkin = app.checkin.lock().await;
        checkin::validate_plan(&checkin.settings, accounts.portfolio.accounts.len())
            .map_err(|_| "Saved schedule overflows the check-in cycle.")?;
    }
    app.isolation_ready.store(
        if app.config.container_mode {
            supervisor::available().await
        } else {
            helper::isolation_available().await
        },
        std::sync::atomic::Ordering::SeqCst,
    );
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| "Cannot bind local server.")?;
    checkin::scheduler(&app);
    println!("AnyRouter Manager listening at http://{bind}");
    serve_runtime(app, listener).await
}

async fn serve_runtime(
    app: Arc<app::App>,
    listener: tokio::net::TcpListener,
) -> Result<(), &'static str> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, app::router(app.clone())).with_graceful_shutdown(async {
        let _ = rx.await;
    });
    let mut server = Box::pin(std::future::IntoFuture::into_future(server));
    tokio::select! {
        result = &mut server => {
            app.shutdown().await;
            app.shutdown_logs().await;
            return result.map_err(|_| "Local server failed.");
        },
        _ = termination() => {}
    }
    // Stop new jobs before signaling HTTP drain, then reap cancelled helpers.
    app.stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        let cleanup = tokio::time::timeout(Duration::from_secs(10), app.shutdown());
        let (result, _) = tokio::join!(&mut server, cleanup);
        result
    })
    .await;
    drop(server);
    app.shutdown_logs().await;
    Ok(())
}

async fn termination() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM registration");
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
}
