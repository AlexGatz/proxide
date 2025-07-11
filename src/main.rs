use clap::{Arg, Command};
mod client;
mod config;
mod proxy;
use config::AppConfig;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt::init();

    let cfg = Command::new("proxide")
        .version("0.1.0")
        .author("Alex Gatz <alex@example.com>")
        .about("A simple reverse proxy built with Rust 🦀")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .env("PROXIDE_CONF")
                .default_value("config.yaml")
                .help("Path to .yaml based configuration file."),
        )
        .get_matches();

    let config_path = cfg
        .get_one::<String>("config")
        .expect("The supplied config path was invalid.")
        .to_owned();

    let initial_config = AppConfig::load_from(&config_path)
        .await
        .expect("Failed to load initial config.");

    let (tx, rx) = watch::channel(());
    let shutdown_tx = Arc::new(Mutex::new(tx));
    let shutdown_rx = rx.clone();

    let mut server_set = proxy::spawn_servers(initial_config, shutdown_rx.clone()).await;

    let (reload_tx, mut reload_rx) = mpsc::channel(1);
    tokio::spawn(config::watch_config(config_path.clone(), reload_tx));

    while reload_rx.recv().await.is_some() {
        println!("Reloading config...");
        match AppConfig::load_from(&config_path).await {
            Ok(new_config) => {
                let tx_guard = shutdown_tx.lock().await;
                tx_guard.send(()).unwrap();

                while server_set.join_next().await.is_some() {
                    println!("Awaiting server JoinSet...")
                }

                let (new_tx, new_rx) = watch::channel(());
                server_set = proxy::spawn_servers(new_config, new_rx).await;

                let mut tx_guard = shutdown_tx.lock().await;
                *tx_guard = new_tx;

                println!("Config reloaded successfully!");
            }
            Err(e) => eprintln!("Failed to reload config: {:?}", e),
        }
    }
}
