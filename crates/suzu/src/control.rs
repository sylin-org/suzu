//! Local UDP control messages.
//!
//! `suzu pause` / `suzu resume` send a single datagram to the
//! Resident; it toggles an in-memory flag and acknowledges the request. The state is
//! only in the running process and is discarded when the process exits. The
//! acknowledgement lets the CLI report when no Resident is available.

use crate::resident::devices::DevicesCmd;
use crate::resident::notifications::NotificationCmd;
use anyhow::{anyhow, bail};
use std::time::Duration;
use tokio::net::UdpSocket;

pub const CONTROL_PORT: u16 = 7898; // S-U-Z-U on a phone keypad

/// Receive control datagrams, dispatch commands, and return acknowledgements.
pub async fn listen(
    tx: tokio::sync::mpsc::Sender<DevicesCmd>,
    notifications: tokio::sync::mpsc::Sender<NotificationCmd>,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", CONTROL_PORT)).await?;
    println!(
        "[control] listening on 127.0.0.1:{CONTROL_PORT} — `suzu pause` / `suzu resume`"
    );
    let mut buf = [0u8; 1024]; // The show command carries text.
    loop {
        let (n, peer) = socket.recv_from(&mut buf).await?;
        let raw = String::from_utf8_lossy(&buf[..n]).trim().to_string();
        let msg = raw.to_lowercase(); // commands match case-blind; payloads keep their case
        let reply: String = match msg.as_str() {
            _ if msg.starts_with("say ") => {
                // Command grammar (ADR-0006): [port] [signal] [text…].
                // Resolve a port by exact name or unique suffix. Commands without
                // a port are broadcast through the display-event rate limiter.
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
                                format!("ok say — sent to {port}")
                            }
                            Ok(Some(Err(e))) => format!("err {e:#}"),
                            _ => "err the resident did not answer".to_string(),
                        }
                    }
                    Some(Err(reason)) => format!("err {reason}"),
                    None => {
                        // Broadcast subject to the display-event rate limit.
                        let kind = parsed
                            .signal
                            .unwrap_or_else(|| "transition".to_string());
                        let urgency =
                            crate::resident::devices::urgency_for(&kind);
                        if notifications
                            .send(NotificationCmd::submit("cli", &kind, parsed.text, urgency))
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
                // Example: suzu show INFO.disk "Disk at 50%".
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
                if notifications
                    .send(NotificationCmd::submit("visitor", &kind, Some(label), urgency))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                "ok show".to_string()
            }
            "pause" => {
                // the send_control's ack is the UDP reply below; the command's
                // This command does not need a second reply channel.
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

/// Send a control command and wait for its acknowledgement.
pub async fn send_control(word: &str) -> anyhow::Result<()> {
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
            println!("paused — device streaming stopped");
            println!("serve keeps running; the ports are free for `suzu screenshot`");
            println!("`suzu resume` restarts the stream");
        }
        "ok resume" => {
            println!("resumed — device sessions reopened and current metrics were republished");
        }
        "ok show" => {
            println!("shown — event sent to connected devices");
        }
        "ok say" => {
            println!("sent — targeted event delivered to the device");
        }
        a if a.starts_with("ok say") => {
            println!("{a}");
        }
        a if a.starts_with("err") => return Err(anyhow!("{a}")),
        other => return Err(anyhow!("unexpected ack: {other}")),
    }
    Ok(())
}
