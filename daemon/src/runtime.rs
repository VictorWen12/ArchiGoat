//! This runtime owns one loopback listener and keeps it serving until a live sibling owns the port.

use crate::{
    Config, DaemonState, api,
    proof::{self, ChallengeResponse},
};
use serde::Serialize;
use std::io::ErrorKind;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    time::{Duration, sleep, timeout},
};

// ChallengeBody reveals only a fresh nonce before the existing process proves its identity.
#[derive(Serialize)]
struct ChallengeBody<'a> {
    client_nonce: &'a str,
}

// ExitBody claims the port for this build's newer protocol under the shared installation key.
#[derive(Serialize)]
struct ExitBody<'a> {
    client_nonce: &'a str,
    server_nonce: &'a str,
    protocol: u16,
    proof: &'a str,
}

// Ownership tells startup whether this process served or found an existing owner.
enum Ownership {
    // Served means this process owned the port and its serve loop ended.
    Served,
    // SiblingOwns means a live current ArchiGoat keeps the port and this process may exit.
    SiblingOwns,
}

// Autostart serves forever and leaves a healthy current owner alone.
pub async fn run_autostart(config: Config) -> Result<(), String> {
    keep_serving(config).await
}

// KeepServing binds one daemon lifetime to its spawning app on macOS and test Unix runs.
async fn keep_serving(config: Config) -> Result<(), String> {
    // Every macOS daemon belongs to the visible app that launched it.
    #[cfg(target_os = "macos")]
    crate::keepalive::watch_parent();
    // Non-product Unix test daemons also die with their spawning process when explicitly requested.
    #[cfg(all(unix, not(target_os = "macos")))]
    if crate::keepalive::disabled() {
        crate::keepalive::watch_parent();
    }
    // A spawned attempt turns even a panic into an observable unclean exit instead of a silent death.
    let attempt = tokio::spawn(attempt_ownership(config));
    match attempt.await {
        Ok(Ok(Ownership::SiblingOwns)) => Ok(()),
        // Reviving in-process would run a second generation beside leaked tasks from the first.
        Ok(Ok(Ownership::Served)) => {
            crate::trace::line("serve ended");
            Err("Serve ended".to_owned())
        }
        Ok(Err(reason)) => {
            crate::trace::line(&format!("serve failed: {reason}"));
            Err(reason)
        }
        Err(panic) => {
            crate::trace::line(&format!("serve panicked: {panic}"));
            Err(format!("Serve panicked: {panic}"))
        }
    }
}

// AttemptOwnership binds or negotiates the port once and serves while this process owns it.
async fn attempt_ownership(config: Config) -> Result<Ownership, String> {
    match TcpListener::bind(config.bind).await {
        Ok(listener) => {
            serve(listener, config).await?;
            Ok(Ownership::Served)
        }
        Err(error) if error.kind() == ErrorKind::AddrInUse => match supersede(&config).await? {
            Some(listener) => {
                serve(listener, config).await?;
                Ok(Ownership::Served)
            }
            None => Ok(Ownership::SiblingOwns),
        },
        Err(error) => Err(format!("Could not listen on {}: {error}", config.bind)),
    }
}

// Supersede rebinds only after a proven stale owner acknowledged exit; anything else keeps the port.
async fn supersede(config: &Config) -> Result<Option<TcpListener>, String> {
    let Some(path) = config.state_file.as_deref() else {
        return Ok(None);
    };
    let Ok(secret) = DaemonState::load_instance_secret(path) else {
        return Ok(None);
    };
    if send_exit(config, &secret).await.is_err() {
        return Ok(None);
    }
    // The stale owner exits asynchronously; bounded rebinding absorbs its shutdown window.
    for _ in 0..20 {
        sleep(Duration::from_millis(100)).await;
        match TcpListener::bind(config.bind).await {
            Ok(listener) => return Ok(Some(listener)),
            Err(error) if error.kind() == ErrorKind::AddrInUse => {}
            Err(error) => return Err(format!("Could not listen on {}: {error}", config.bind)),
        }
    }
    Ok(None)
}

// SendExit authenticates the owner challenge before asking a stale owner to yield the port.
async fn send_exit(config: &Config, secret: &str) -> Result<(), String> {
    let client_nonce = proof::nonce()?;
    let challenge_body = serde_json::to_string(&ChallengeBody {
        client_nonce: &client_nonce,
    })
    .map_err(|error| format!("Could not encode exit challenge: {error}"))?;
    let (status, body) = send_json(config, "/internal/challenge", &challenge_body).await?;
    if status != 200 {
        return Err("Port owner is not a verifiable ArchiGoat".to_owned());
    }
    let challenge: ChallengeResponse = serde_json::from_slice(&body)
        .map_err(|_| "Port owner returned an invalid challenge".to_owned())?;
    if !proof::valid_nonce(&challenge.server_nonce)
        || !proof::verify_server(
            secret,
            &client_nonce,
            &challenge.server_nonce,
            &challenge.proof,
        )
    {
        return Err("Port owner could not prove ArchiGoat identity".to_owned());
    }
    let keyed_proof = proof::exit_proof(
        secret,
        &client_nonce,
        &challenge.server_nonce,
        api::PROTOCOL,
    )?;
    let exit_body = serde_json::to_string(&ExitBody {
        client_nonce: &client_nonce,
        server_nonce: &challenge.server_nonce,
        protocol: api::PROTOCOL,
        proof: &keyed_proof,
    })
    .map_err(|error| format!("Could not encode exit claim: {error}"))?;
    let (status, _) = send_json(config, "/internal/exit", &exit_body).await?;
    if status == 204 {
        Ok(())
    } else {
        Err("Live ArchiGoat is current".to_owned())
    }
}

// Serve restores durable Work and Provider observation before accepting direct browser actions.
async fn serve(listener: TcpListener, config: Config) -> Result<(), String> {
    let state = DaemonState::new(config)?;
    // Every new port owner leaves one birth line so a later death is diagnosable.
    crate::trace::line("serving");
    // The keepalive task guarantees a revival tick even when this whole process is killed.
    #[cfg(windows)]
    crate::windows::keepalive::ensure();
    // Wake observation restores relay state after the Mac resumes without adding persistence.
    #[cfg(target_os = "macos")]
    crate::keepalive::observe_wake();
    crate::process::resume(state.clone());
    tokio::spawn(crate::account_relay::run(state.clone()));
    // Maintenance runs on its own slow clock, never inside the relay loop parked before registration.
    tokio::spawn(crate::update::run(state.clone()));
    // A saved Provider is re-proven silently, so this probe claims no connect admission from the owner.
    if let Some(provider) = state.reconnect_provider().await
        && let Some(epoch) = state.begin_observe(provider).await
    {
        let observing = state.clone();
        tokio::spawn(async move {
            crate::connection::observe(observing, provider, epoch).await;
        });
    }
    axum::serve(listener, crate::local::app(state))
        .await
        .map_err(|error| format!("Local ArchiGoat stopped: {error}"))
}

// SendJson uses the minimum local HTTP exchange needed for authenticated port-owner handoff.
async fn send_json(config: &Config, path: &str, body: &str) -> Result<(u16, Vec<u8>), String> {
    let exchange = async {
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            config.bind,
            body.len(),
            body
        );
        let mut stream = TcpStream::connect(config.bind)
            .await
            .map_err(|error| format!("Could not connect to live ArchiGoat: {error}"))?;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| format!("Could not send owner request: {error}"))?;
        let mut response = Vec::new();
        stream
            .take(8_192)
            .read_to_end(&mut response)
            .await
            .map_err(|error| format!("Could not read owner response: {error}"))?;
        parse_response(&response)
    };
    // A bounded exchange keeps launchd-started probes from stacking against a starved owner forever.
    timeout(Duration::from_secs(10), exchange)
        .await
        .map_err(|_| "Live ArchiGoat did not answer the handoff".to_owned())?
}

// ParseResponse extracts only one HTTP status and body from the bounded loopback reply.
fn parse_response(response: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "Live ArchiGoat response is invalid".to_owned())?;
    let head = std::str::from_utf8(&response[..split])
        .map_err(|_| "Live ArchiGoat response is invalid".to_owned())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "Live ArchiGoat status is invalid".to_owned())?;
    Ok((status, response[split + 4..].to_vec()))
}
