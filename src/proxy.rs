use crate::client::HttpClient;
use crate::config::AppConfig;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, Uri},
    response::IntoResponse,
};
use axum_server::{Handle, bind};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    sync::{Mutex, watch},
    task::JoinSet,
};

#[derive(Clone)]
struct ProxyState {
    name: String,
    upstream: String,
    client: Arc<Mutex<HttpClient<Body>>>,
}

const MAX_REQUEST_SIZE: usize = 1 * 1024 * 1024; // 1 MiB
const MAX_CLIENT_CONN_RETRIES: usize = 10;

async fn proxy_handler(
    State(state): State<ProxyState>,
    mut req: Request<Body>,
) -> impl IntoResponse {
    let uri = format!(
        "http://{}{}",
        state.upstream,
        req.uri()
            .path_and_query()
            .map(|x| x.as_str())
            .unwrap_or("/")
    );

    let name = &state.name;
    println!("APP: {name} URI: {uri}");

    *req.uri_mut() = uri.parse::<Uri>().expect("Failed to parse URI");

    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_SIZE).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Failed to buffer request body").into_response();
        }
    };

    let mut client_guard = state.client.lock().await;
    let req = Request::from_parts(parts.clone(), Body::from(body_bytes.clone()));

    match client_guard.send(req).await {
        Ok(resp) => resp.into_response(),
        Err(_) => {
            let client_addr = state
                .upstream
                .parse::<SocketAddr>()
                .expect("Proxy handler failed to parse URI into SocketAddr");

            if client_guard
                .reconnect_with_backoff(client_addr, MAX_CLIENT_CONN_RETRIES)
                .await
                .is_ok()
            {
                let retry_req = Request::from_parts(parts, Body::from(body_bytes));
                match client_guard.send(retry_req).await {
                    Ok(resp) => resp.into_response(),
                    Err(_) => (StatusCode::GATEWAY_TIMEOUT, "Upstream unreachable").into_response(),
                }
            } else {
                (StatusCode::BAD_GATEWAY, "Failed to reconnect").into_response()
            }
        }
    }
}

pub async fn spawn_servers(config: AppConfig, shutdown_rx: watch::Receiver<()>) -> JoinSet<()> {
    let mut set = JoinSet::new();

    for (name, cfg) in config.servers {
        let server_addr = cfg
            .listen
            .parse::<SocketAddr>()
            .expect("Server addr failed to parse");
        let client_addr = cfg
            .upstream
            .parse::<SocketAddr>()
            .expect("Client addr failed to parse");
        let client = Arc::new(Mutex::new(
            HttpClient::connect(client_addr)
                .await
                .expect("Failed to establish client."),
        ));
        let app_name = name.clone();

        let state = ProxyState {
            name: app_name,
            upstream: cfg.upstream.clone(),
            client,
        };

        let mut shutdown_rx = shutdown_rx.clone();

        set.spawn(async move {
            let router = Router::new().fallback(proxy_handler).with_state(state);
            println!("Server {name}: Listening on {server_addr} Upstream {client_addr}");

            let handle = Handle::new();
            // TODO: Validate graceful shutdown...
            let shutdown_handle = handle.clone();

            tokio::spawn(async move {
                shutdown_rx.changed().await.ok();
                println!("Shutting down {name} at {server_addr}");
                shutdown_handle.shutdown();
            });

            // TODO: add branch for bind vs bind_rustls based on AppConfig
            bind(server_addr)
                .handle(handle)
                .serve(router.into_make_service())
                .await
                .unwrap();
        });
    }
    set
}
