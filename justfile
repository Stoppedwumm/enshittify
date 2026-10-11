# Cutroom task runner — https://github.com/casey/just
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

relay_addr := "127.0.0.1:7878"
demo_dir := "target/demo"

default:
    @just --list

# Build everything (CLI FFmpeg backend; no FFmpeg dev libraries needed).
build:
    cargo build --workspace

# Build with FFmpeg linked natively (needs FFmpeg 6–8 dev libraries).
build-native:
    cargo build --workspace --features cutroom/ffmpeg

# Run all tests (media tests are skipped when ffmpeg is not on PATH).
test:
    cargo test --workspace

# Full acceptance run: 10k-command convergence in release mode.
test-full $CUTROOM_CONVERGENCE_OPS="10000":
    cargo test --release -p cutroom-tests --test convergence

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

# Launch the editor (optionally with a project path).
run *ARGS:
    cargo run -p cutroom -- {{ARGS}}

# Start the relay server.
run-relay:
    cargo run -p cutroom-relay -- --addr {{relay_addr}} --db target/cutroom-relay.sqlite

# Generate a demo project with test media and open it.
demo:
    cargo run -p cutroom-ui --example demo_project -- {{demo_dir}}
    cargo run -p cutroom -- {{demo_dir}}/Demo.cutroom

# Relay + two editors on one machine: Ana shares the demo project, Ben joins.
[unix]
demo-two-clients:
    #!/usr/bin/env sh
    set -e
    cargo build -p cutroom -p cutroom-relay
    cargo run -q -p cutroom-ui --example demo_project -- {{demo_dir}}
    ./target/debug/cutroom-relay --addr {{relay_addr}} --memory & RELAY=$!
    trap 'kill $RELAY 2>/dev/null' EXIT
    sleep 1
    CUTROOM_CONFIG_DIR={{demo_dir}}/cfg-ana ./target/debug/cutroom {{demo_dir}}/Demo.cutroom \
        --relay ws://{{relay_addr}}/ws --room demo --name Ana & A=$!
    sleep 2
    CUTROOM_CONFIG_DIR={{demo_dir}}/cfg-ben ./target/debug/cutroom {{demo_dir}}/Ben.cutroom \
        --relay ws://{{relay_addr}}/ws --room demo --name Ben & B=$!
    wait $A $B

[windows]
demo-two-clients:
    cargo build -p cutroom -p cutroom-relay
    cargo run -q -p cutroom-ui --example demo_project -- {{demo_dir}}
    $relay = Start-Process -PassThru ./target/debug/cutroom-relay.exe -ArgumentList "--addr {{relay_addr}} --memory"
    Start-Sleep 1
    $env:CUTROOM_CONFIG_DIR="{{demo_dir}}/cfg-ana"; $a = Start-Process -PassThru ./target/debug/cutroom.exe -ArgumentList "{{demo_dir}}/Demo.cutroom --relay ws://{{relay_addr}}/ws --room demo --name Ana"
    Start-Sleep 2
    $env:CUTROOM_CONFIG_DIR="{{demo_dir}}/cfg-ben"; $b = Start-Process -PassThru ./target/debug/cutroom.exe -ArgumentList "{{demo_dir}}/Ben.cutroom --relay ws://{{relay_addr}}/ws --room demo --name Ben"
    Wait-Process -Id $a.Id,$b.Id; Stop-Process -Id $relay.Id
