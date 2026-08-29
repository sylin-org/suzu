//! The control chirp — one-packet UDP on localhost.
//!
//! `suzu pause` / `suzu resume` shout a single datagram at the
//! Resident; it toggles an in-memory flag and acks. The state lives
//! only in the running process: nothing persists, nothing survives a
//! crash, and a forgotten pause ends the moment serve does. The ack
//! lets the CLI tell the truth — a chirp into the void answers
//! "is `suzu serve` running?" instead of pretending.

use crate::resident::devices::DevicesCmd;
use crate::resident::moments::MomentsCmd;
use anyhow::{anyhow, bail};
use std::time::Duration;
use tokio::net::UdpSocket;

pub const CONTROL_PORT: u16 = 7898; // S-U-Z-U on a phone keypad

/// The Resident's ear: chirps in, commands out, acks back.
pub async fn listen(
    mut tx: tokio::sync::mpsc::Sender<DevicesCmd>,
    moments: tokio::sync::mpsc::Sender<MomentsCmd>,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", CONTROL_PORT)).await?;
    println!(
        "[control] listening on 127.0.0.1:{CONTROL_PORT} — `suzu pause` / `suzu resume`"
    );
    let mut buf = [0u8; 1024]; // show carries text
    loop {
        let (n, peer) = socket.recv_from(&mut buf).await?;
        let msg = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
        let reply = match msg.as_str() {
            _ if msg.starts_with("show ") => {
                // suzu show INFO.disk "Disk at 50%" — a moment for faces
                let spec = msg["show ".len()..].trim();
                let (tag, text) = spec
                    .split_once(' ')
                    .unwrap_or((spec, ""));
                let kind = tag.split('.').next().unwrap_or("note").to_uppercase();
                let urgency = match kind.as_str() {
                    "WARN" => 3,
                    "ALERT" | "CRIT" => 5,
                    _ => 1,
                };
                let label = if text.is_empty() {
                    tag.to_string()
                } else {
                    text.to_string()
                };
                if moments
                    .send(MomentsCmd::tell("visitor", &kind, Some(label), urgency))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                "ok show"
            }
            "pause" => {
                if tx.send(DevicesCmd::Pause).await.is_err() {
                    return Ok(());
                }
                "ok pause"
            }
            "resume" => {
                if tx.send(DevicesCmd::Resume).await.is_err() {
                    return Ok(());
                }
                "ok resume"
            }
            _ => "err unknown",
        };
        let _ = socket.send_to(reply.as_bytes(), peer).await;
    }
}

/// The CLI's mouth: chirp, wait for the ack, speak honestly.
pub async fn chirp(word: &str) -> anyhow::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0").await?;
    socket
        .send_to(word.as_bytes(), ("127.0.0.1", CONTROL_PORT))
        .await?;
    let mut buf = [0u8; 32];
    let ack = match tokio::time::timeout(
        Duration::from_secs(1),
        socket.recv_from(&mut buf),
    )
    .await
    {
        Ok(Ok((n, _))) => String::from_utf8_lossy(&buf[..n]).to_string(),
        _ => bail!("no answer from the resident — is `suzu serve` running?"),
    };
    match ack.trim() {
        "ok pause" => {
            println!("paused — the house stops streaming; faces fall idle into their animations");
            println!("serve keeps running; the ports are free for `suzu screenshot`");
            println!("`suzu resume` restarts the stream");
        }
        "ok resume" => {
            println!("resumed — sessions re-open and faces redress as the ground republishes");
        }
        "ok show" => {
            println!("shown — faces ring the moment; the band returns to the house label");
        }
        other => return Err(anyhow!("unexpected ack: {other}")),
    }
    Ok(())
}
