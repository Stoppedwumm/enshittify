//! `cutroom-relay [--addr 127.0.0.1:7878] [--db relay.sqlite]`

use std::net::SocketAddr;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut addr: SocketAddr = "127.0.0.1:7878".parse()?;
    let mut db: Option<PathBuf> = Some("cutroom-relay.sqlite".into());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => {
                addr = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--addr needs a value"))?
                    .parse()?
            }
            "--db" => {
                db = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?
                        .into(),
                )
            }
            "--memory" => db = None,
            "-h" | "--help" => {
                println!("cutroom-relay [--addr HOST:PORT] [--db FILE | --memory]");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let relay = cutroom_relay::Relay::new(db.as_deref())?;
    tokio::runtime::Runtime::new()?.block_on(cutroom_relay::serve(addr, relay))
}
