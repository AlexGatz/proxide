use hyper::{
    Request, Response,
    body::Body,
    client::conn::http1::{SendRequest, handshake},
};
use hyper_util::rt::TokioIo;
use tokio::{
    net::{TcpStream, ToSocketAddrs},
    task::JoinHandle,
    time::{Duration, sleep},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct HttpClient<B> {
    sender: SendRequest<B>,
    // TODO: Validate graceful connection shutdown to see if this is even needed.
    shutdown_handle: Option<JoinHandle<()>>,
}

impl<B> HttpClient<B>
where
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    pub async fn connect(addr: impl tokio::net::ToSocketAddrs) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        let io = TokioIo::new(stream);

        let (sender, conn) = handshake(io).await?;
        let handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("connection error: {:?}", e);
            }
        });

        Ok(Self {
            sender,
            shutdown_handle: Some(handle),
        })
    }

    pub async fn reconnect_with_backoff(
        &mut self,
        addr: impl ToSocketAddrs + Copy,
        max_retries: usize,
    ) -> Result<()> {
        let mut delay = Duration::from_millis(100);

        for attempt in 1..=max_retries {
            match TcpStream::connect(addr).await {
                Ok(stream) => {
                    let io = TokioIo::new(stream);
                    if let Ok((sender, conn)) = handshake(io).await {
                        let handle = tokio::spawn(async move {
                            if let Err(e) = conn.await {
                                eprintln!("connection error: {:?}", e);
                            }
                        });
                        self.sender = sender;
                        self.shutdown_handle = Some(handle);
                        return Ok(());
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Reconnect attempt {}/{} failed: {}",
                        attempt, max_retries, e
                    );
                    sleep(delay).await;
                    delay *= 2;
                }
            }
        }

        Err("Failed to reconnect after max retries".into())
    }

    pub async fn send(&mut self, req: Request<B>) -> Result<Response<hyper::body::Incoming>> {
        Ok(self.sender.send_request(req).await?)
    }
}
