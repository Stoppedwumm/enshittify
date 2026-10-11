//! Creates a demo project with generated media.
//!
//! `cargo run -p cutroom-ui --example demo_project -- OUT_DIR`

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo".into());
    let p = cutroom_ui::demo::create_demo_project(std::path::Path::new(&out))?;
    println!("{}", p.display());
    Ok(())
}
