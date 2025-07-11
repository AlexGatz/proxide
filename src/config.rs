use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::{fs, sync::mpsc};

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub listen: String,
    pub upstream: String,
}

// TODO: Add tls config
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub servers: HashMap<String, ServerConfig>,
}

impl AppConfig {
    pub async fn load_from(path: impl AsRef<str>) -> Result<Self> {
        let config_str = fs::read_to_string(path.as_ref()).await?;
        Ok(serde_yaml::from_str(&config_str)?)
    }
}

pub async fn watch_config(
    path: impl AsRef<str>,
    reload_tx: mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let (notify_tx, mut notify_rx) = mpsc::channel(1);
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, _>| {
            if res.is_ok() {
                let _ = notify_tx.try_send(());
            }
        },
        notify::Config::default(),
    )?;
    watcher.watch(
        std::path::Path::new(path.as_ref()),
        RecursiveMode::NonRecursive,
    )?;

    while notify_rx.recv().await.is_some() {
        let _ = reload_tx.send(()).await;
    }

    Ok(())
}
