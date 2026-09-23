use std::path::PathBuf;
use yourself_server::service::{app_router, App};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("YourSelf: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut port = 4317u16;
    let mut data_dir = PathBuf::from(".local");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse()?;
                i += 2;
            }
            "--data-dir" if i + 1 < args.len() => {
                data_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--help" => {
                println!(
                    "yourself-server [--port 4317] [--data-dir .local]\nListens on 127.0.0.1 only."
                );
                return Ok(());
            }
            _ => return Err("Unknown argument; use --help".into()),
        }
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let actual_port = listener.local_addr()?.port();
    let app = App::open(&data_dir, actual_port).await?;
    let feishu = yourself_server::feishu::start(app.clone());
    let heartbeat = yourself_server::heartbeat::start(app.clone());
    println!("YourSelf is ready at http://127.0.0.1:{actual_port}");
    axum::serve(listener, app_router(app.clone()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    feishu.abort();
    heartbeat.abort();
    app.mind.shutdown().await?;
    Ok(())
}
