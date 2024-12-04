use axum::routing::get;
use axum::{serve, Router};
use std::process::exit;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::{runtime, select, signal};
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer())
        .init();

    let mut manager = Manager::new()?;

    manager.start()?;

    info!("starting");

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

        select! {
            _ = ctrl_c => {
                info!("receive ctrl_c to shutting down server");
                manager.stop().expect("failed to stop manager");
            },
            _ = terminate => {
                error!("signal handler exited unexpectedly")
            },
        }

    Ok(())
}

struct ApiService {
    cancel_token: CancellationToken,
}

impl ApiService {
    fn new(token: CancellationToken) -> Result<Self, anyhow::Error> {
        Ok(Self {
            cancel_token: token,
        })
    }

    fn start(&self) -> Result<(), anyhow::Error> {
        info!("starting api service");
        let token = self.cancel_token.clone();
        tokio::spawn(async {
            let app = Router::new()
                .route("/health", get(Self::health))
                // request trace
                .layer(TraceLayer::new_for_http());

            // Create a `TcpListener` using tokio.
            let addr = format!("{}:{}", "0.0.0.0", "8080");
            let result = TcpListener::bind(addr).await;
            match result {
                Ok(listener) => {
                    info!("listening on {:?}", listener.local_addr().unwrap());
                    serve(listener, app)
                        .with_graceful_shutdown(async move {
                           loop {
                               if token.is_cancelled() {
                                   info!("cancelled token");
                                   break;
                               }
                           }
                        })
                        .await.expect("serve failed");
                }
                Err(err) => {
                    error!("{}", err);
                }
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), anyhow::Error> {
        info!("stopping api service");
        self.cancel_token.cancel();
        Ok(())
    }

    async fn health() -> &'static str {
        "OK"
    }
}

struct Manager {
    parent_token: CancellationToken,
    api_service: Arc<RwLock<ApiService>>,
}

impl Manager {
    fn new() -> Result<Self, anyhow::Error> {
        let parent_token = CancellationToken::new();
        let api_service = ApiService::new(parent_token.clone())?;
        Ok(Self {
            parent_token,
            api_service: Arc::new(RwLock::new(api_service)),
        })
    }

    fn start(&mut self) -> Result<(), anyhow::Error> {
        info!("start all services");
        self.start_api_service()
    }

    fn stop(&mut self) -> Result<(), anyhow::Error> {
        info!("stop all services");
        self.stop_api_service()
    }

    fn start_api_service(&mut self) -> Result<(), anyhow::Error> {
        let api_service = self.api_service.clone();
        let lock = api_service.try_write();
        match lock {
            Ok(writer) => {
                writer.start()?;
            }
            Err(err) => {
                return Err(anyhow::anyhow!(err));
            }
        }
        Ok(())
    }

    fn stop_api_service(&mut self) -> Result<(), anyhow::Error> {
        let api_service = self.api_service.clone();
        let lock = api_service.try_write();
        match lock {
            Ok(writer) => {
                writer.stop()?;
            }
            Err(err) => {
                return Err(anyhow::anyhow!(err));
            }
        }
        Ok(())
    }
}
