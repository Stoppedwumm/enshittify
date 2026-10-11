//! Cutroom desktop app.
//!
//! ```text
//! cutroom [PROJECT.cutroom] [--relay ws://HOST:PORT/ws --room ROOM] [--name NAME] [--no-audio]
//! ```

// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use cutroom_ui::AppOptions;

fn parse_args() -> anyhow::Result<AppOptions> {
    let mut o = AppOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{name} needs a value"))
        };
        match a.as_str() {
            "--relay" => o.relay = Some(val("--relay")?),
            "--room" => o.room = Some(val("--room")?),
            "--name" => o.user_name = Some(val("--name")?),
            "--no-audio" => o.no_audio = true,
            "-h" | "--help" => {
                println!(
                    "cutroom [PROJECT.cutroom] [--relay URL --room ROOM] [--name NAME] [--no-audio]"
                );
                std::process::exit(0);
            }
            p if !p.starts_with('-') => o.project = Some(p.into()),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(o)
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .init();
    let opts = parse_args()?;
    cutroom_ui::run(opts).map_err(|e| anyhow::anyhow!("{e}"))
}
