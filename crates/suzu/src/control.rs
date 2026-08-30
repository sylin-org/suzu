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
    tx: tokio::sync::mpsc::Sender<DevicesCmd>,
    moments: tokio::sync::mpsc::Sender<MomentsCmd>,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", CONTROL_PORT)).await?;
    println!(
        "[control] listening on 127.0.0.1:{CONTROL_PORT} — `suzu pause` / `suzu resume`"
    );
    let mut buf = [0u8; 1024]; // show carries text
    loop {
        let (n, peer) = socket.recv_from(&mut buf).await?;
        let raw = String::from_utf8_lossy(&buf[..n]).trim().to_string();
        let msg = raw.to_lowercase(); // commands match case-blind; payloads keep their case
        let reply: String = match msg.as_str() {
            _ if msg.starts_with("say ") => {
                // The sentence grammar (ADR-0006): [port] [signal] [text…].
                // A port resolves against the live enumeration — exact
                // name or unique suffix; prose is a broadcast through
                // the moments budget.
                let sentence = raw["say ".len()..].trim();
                let ports: Vec<String> = crate::enumerate()
                    .into_iter()
                    .map(|e| e.name)
                    .collect();
                let parsed =
                    crate::resident::devices::parse_say(sentence, &ports);
                match parsed.target {
                    Some(Ok(port)) => {
                        let signal =
                            parsed.signal.unwrap_or_else(|| "info".to_string());
                        let (reply_tx, mut reply_rx) = tokio::sync::mpsc::channel(1);
                        if tx
                            .send(DevicesCmd::Say {
                                port: port.clone(),
                                signal,
                                text: parsed.text,
                                reply: reply_tx,
                            })
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            reply_rx.recv(),
                        )
                        .await
                        {
                            Ok(Some(Ok(()))) => {
                                format!("ok say — {port} takes the stage")
                            }
                            Ok(Some(Err(e))) => format!("err {e:#}"),
                            _ => "err the house did not answer".to_string(),
                        }
                    }
                    Some(Err(reason)) => format!("err {reason}"),
                    None => {
                        // broadcast: the moments budget governs visitors
                        let kind = parsed
                            .signal
                            .unwrap_or_else(|| "transition".to_string());
                        let urgency =
                            crate::resident::devices::urgency_for(&kind);
                        if moments
                            .send(MomentsCmd::tell("keeper", &kind, parsed.text, urgency))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                        "ok say (broadcast)".to_string()
                    }
                }
            }
            _ if msg.starts_with("show ") => {
                // suzu show INFO.disk "Disk at 50%" — a moment for faces
                let spec = raw["show ".len()..].trim();
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
                "ok show".to_string()
            }
            "pause" => {
                // the chirp's ack is the UDP reply below; the command's
                // own door reply is not needed here.
                let (reply, _rx) = tokio::sync::mpsc::channel(1);
                if tx.send(DevicesCmd::Pause { reply }).await.is_err() {
                    return Ok(());
                }
                "ok pause".to_string()
            }
            "resume" => {
                let (reply, _rx) = tokio::sync::mpsc::channel(1);
                if tx.send(DevicesCmd::Resume { reply }).await.is_err() {
                    return Ok(());
                }
                "ok resume".to_string()
            }
            _ => "err unknown".to_string(),
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
    let mut buf = [0u8; 256]; // acks may carry reasons
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
        "ok say" => {
            println!("said — the ring rides the wire; the face carries the story");
        }
        a if a.starts_with("ok say") => {
            println!("{a}");
        }
        a if a.starts_with("err") => return Err(anyhow!("{a}")),
        other => return Err(anyhow!("unexpected ack: {other}")),
    }
    Ok(())
}
