use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr = agent_server::listen_addr().unwrap_or_else(|error| panic!("{error}"));
    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    agent_server::serve(listener)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}
