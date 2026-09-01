//! The keeper's terminal window.
//!
//! This is deliberately a client of the Resident, just like Workbench.
//! It reads the same snapshot/roster stream and sends the same typed
//! device actions; no serial or lifecycle rule is duplicated here.

use crate::resident::device::DeviceAction;
use crate::resident::events::{DeviceRow, HouseSnapshot};
use crate::resident::roster::{Individual, Lifecycle};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const RESIDENT: &str = "127.0.0.1:7899";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REPLY_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_REPLY: usize = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Faceplate {
    id: String,
    name: String,
    #[serde(default)]
    blurb: String,
    mount: Option<String>,
    version: Option<String>,
}

enum FaceplateSelection {
    NoneDeclared,
    Selected(String),
    Cancelled,
}

struct ResidentClient;

impl ResidentClient {
    async fn connect(&self) -> Result<TcpStream> {
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(RESIDENT))
            .await
            .context("the Resident did not answer within 2s")?
            .with_context(|| {
                format!(
                    "cannot reach the Suzu Resident at {RESIDENT} — is suzu@<keeper>.service running?"
                )
            })
    }

    async fn events(&self) -> Result<EventStream> {
        let mut stream = self.connect().await?;
        stream
            .write_all(
                b"GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await?;
        EventStream::open(stream).await
    }

    async fn snapshot(&self) -> Result<HouseSnapshot> {
        let mut events = self.events().await?;
        let first = events.next_json().await?;
        if first.get("type").and_then(Value::as_str) != Some("snapshot") {
            bail!("the Resident stream did not open with a snapshot");
        }
        serde_json::from_value(first.get("snapshot").cloned().unwrap_or(Value::Null))
            .context("the Resident snapshot did not match the shared read model")
    }

    async fn faceplates(&self, class: &str) -> Result<Vec<Faceplate>> {
        let (_, body) = self
            .request("GET", &format!("/api/faceplates/{class}"), None)
            .await?;
        serde_json::from_value(body).context("the faceplate list was malformed")
    }

    async fn action(
        &self,
        port: &str,
        action: DeviceAction,
        faceplate: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({});
        if let Some(faceplate) = faceplate {
            body["faceplate"] = json!(faceplate);
        }
        let verb = match action {
            DeviceAction::FactoryReset => "factory-reset",
            _ => action.as_str(),
        };
        let (status, reply) = self
            .request(
                "POST",
                &format!("/api/device/{port}/{verb}"),
                Some(&body.to_string()),
            )
            .await?;
        if !(200..300).contains(&status) {
            bail!(
                "{}",
                reply
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the Resident refused the action")
            );
        }
        Ok(reply)
    }

    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, Value)> {
        let mut stream = self.connect().await?;
        let payload = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        stream.write_all(request.as_bytes()).await?;
        let mut raw = Vec::new();
        tokio::time::timeout(REPLY_TIMEOUT, stream.read_to_end(&mut raw))
            .await
            .context("the Resident reply exceeded 8s")??;
        if raw.len() > MAX_REPLY {
            bail!("the Resident reply exceeded 8 MiB");
        }
        let header_end = find_bytes(&raw, b"\r\n\r\n")
            .ok_or_else(|| anyhow::anyhow!("the Resident returned an invalid HTTP response"))?;
        let head = String::from_utf8_lossy(&raw[..header_end]);
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|word| word.parse::<u16>().ok())
            .ok_or_else(|| anyhow::anyhow!("the Resident returned no HTTP status"))?;
        let body = &raw[header_end + 4..];
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).context("the Resident returned invalid JSON")?
        };
        Ok((status, value))
    }
}

struct EventStream {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl EventStream {
    async fn open(mut stream: TcpStream) -> Result<Self> {
        let mut pending = Vec::new();
        loop {
            if let Some(at) = find_bytes(&pending, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&pending[..at]);
                if !head
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains(" 200 "))
                {
                    bail!("the Resident refused its event stream");
                }
                pending.drain(..at + 4);
                return Ok(Self { stream, pending });
            }
            read_more(&mut stream, &mut pending).await?;
        }
    }

    async fn next_json(&mut self) -> Result<Value> {
        loop {
            if find_bytes(&self.pending, b"\n\n").is_some() {
                if let Some(value) = take_sse_json(&mut self.pending)? {
                    return Ok(value);
                }
                continue;
            }
            read_more(&mut self.stream, &mut self.pending).await?;
        }
    }
}

async fn read_more(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Result<()> {
    let mut chunk = [0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(20), stream.read(&mut chunk))
        .await
        .context("the Resident event stream went quiet")??;
    if n == 0 {
        bail!("the Resident event stream closed");
    }
    pending.extend_from_slice(&chunk[..n]);
    if pending.len() > MAX_REPLY {
        bail!("the Resident event exceeded 8 MiB");
    }
    Ok(())
}

fn take_sse_json(pending: &mut Vec<u8>) -> Result<Option<Value>> {
    let Some(end) = find_bytes(pending, b"\n\n") else {
        return Ok(None);
    };
    let block = String::from_utf8_lossy(&pending[..end]).to_string();
    pending.drain(..end + 2);
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str(&data).context("invalid JSON on the Resident stream")?,
    ))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub async fn run(args: &[String]) -> Result<()> {
    let mut json_output = false;
    let mut force_interactive = false;
    let mut plain = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_output = true,
            "--interactive" => force_interactive = true,
            "--plain" => plain = true,
            "-h" | "--help" => {
                println!("usage: suzu list [--json | --plain | --interactive]");
                println!(
                    "Lists the Resident's compatible devices; a terminal opens the keeper menu."
                );
                return Ok(());
            }
            other => bail!("unknown list option {other:?}"),
        }
    }
    if json_output && force_interactive {
        bail!("--json and --interactive cannot be combined");
    }
    let client = ResidentClient;
    let snapshot = client.snapshot().await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }
    let interactive =
        force_interactive || (!plain && io::stdin().is_terminal() && io::stdout().is_terminal());
    if !interactive {
        print_devices(&snapshot);
        return Ok(());
    }
    interactive_list(&client, snapshot).await
}

async fn interactive_list(client: &ResidentClient, mut snapshot: HouseSnapshot) -> Result<()> {
    loop {
        print_devices(&snapshot);
        if snapshot.devices.is_empty() {
            match prompt("[r] refresh  [q] quit > ")?.as_deref() {
                Some("r") | Some("") => snapshot = client.snapshot().await?,
                _ => return Ok(()),
            }
            continue;
        }
        let answer = prompt("Select a device number, [r] refresh, [q] quit > ")?;
        let Some(answer) = answer else { return Ok(()) };
        match answer.trim() {
            "q" | "quit" => return Ok(()),
            "r" | "" => snapshot = client.snapshot().await?,
            choice => {
                let index = choice
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=snapshot.devices.len()).contains(n));
                if let Some(index) = index {
                    manage_device(client, snapshot.devices[index - 1].clone()).await?;
                    snapshot = client.snapshot().await?;
                } else {
                    println!("Choose 1–{}, r, or q.", snapshot.devices.len());
                }
            }
        }
    }
}

fn print_devices(snapshot: &HouseSnapshot) {
    println!(
        "\nSuzu {} · {} compatible device{}",
        snapshot.service.version,
        snapshot.devices.len(),
        if snapshot.devices.len() == 1 { "" } else { "s" }
    );
    if snapshot.devices.is_empty() {
        println!("  No compatible devices detected.");
        return;
    }
    for (index, device) in snapshot.devices.iter().enumerate() {
        let lifecycle = device.lifecycle.as_deref().unwrap_or("new").to_uppercase();
        let class = device.class.as_deref().unwrap_or("unknown class");
        let dress = match (&device.faceplate, &device.mount) {
            (Some(face), Some(mount)) => format!(" · {face} ({mount})"),
            (Some(face), None) => format!(" · {face}"),
            _ => String::new(),
        };
        println!(
            "  {:>2}. {:<10} {:<16} {}{} · v{}",
            index + 1,
            lifecycle,
            device.port,
            class,
            dress,
            device.version.as_deref().unwrap_or("?")
        );
    }
}

async fn manage_device(client: &ResidentClient, device: DeviceRow) -> Result<()> {
    println!(
        "\n{} · {}",
        device.port,
        device.class.as_deref().unwrap_or("unknown class")
    );
    if device.actions.is_empty() {
        println!("Maintenance owns this device; follow its progress in the log and refresh.");
        return Ok(());
    }
    for (index, action) in device.actions.iter().enumerate() {
        println!("  {}. {}", index + 1, action_label(*action, &device));
    }
    println!("  b. Back to devices");
    let Some(answer) = prompt("Choose an action > ")? else {
        return Ok(());
    };
    if matches!(answer.trim(), "b" | "back" | "") {
        return Ok(());
    }
    let Some(action) = answer
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=device.actions.len()).contains(n))
        .map(|n| device.actions[n - 1])
    else {
        println!("No such action.");
        return Ok(());
    };

    let faceplate_choice = match action {
        DeviceAction::Install => choose_faceplate(client, device.class.as_deref()).await?,
        DeviceAction::Update if device.lifecycle.as_deref() != Some("new") => {
            choose_faceplate(client, device.class.as_deref()).await?
        }
        _ => FaceplateSelection::NoneDeclared,
    };
    let faceplate = match faceplate_choice {
        FaceplateSelection::NoneDeclared => None,
        FaceplateSelection::Selected(id) => Some(id),
        FaceplateSelection::Cancelled => return Ok(()),
    };
    if matches!(action, DeviceAction::Install | DeviceAction::Update)
        && !confirm(
            "Proceed? The identity is preserved and admission must pass before LIVE returns",
        )?
    {
        return Ok(());
    }
    if action == DeviceAction::FactoryReset {
        let answer =
            prompt("Factory reset erases and rebuilds flash. Type 'factory' to continue > ")?;
        if answer.as_deref().map(str::trim) != Some("factory") {
            println!("Factory reset cancelled.");
            return Ok(());
        }
    }

    let reply = client
        .action(&device.port, action, faceplate.as_deref())
        .await?;
    println!(
        "{}",
        reply
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Action accepted.")
    );
    if matches!(
        action,
        DeviceAction::Install | DeviceAction::Update | DeviceAction::FactoryReset
    ) {
        let Some(device_id) = device.device_id.as_deref() else {
            println!("The saga is running; refresh the list to see its result.");
            return Ok(());
        };
        follow_maintenance(client, device_id).await?;
    }
    Ok(())
}

fn action_label(action: DeviceAction, device: &DeviceRow) -> &'static str {
    match action {
        DeviceAction::Pause => "Pause",
        DeviceAction::Resume => "Resume",
        DeviceAction::Identify => "Identify on the desk",
        DeviceAction::Install if device.lifecycle.as_deref() == Some("new") => "Install firmware",
        DeviceAction::Install => "Reinstall firmware",
        DeviceAction::Update if device.lifecycle.as_deref() == Some("new") => "Update dress",
        DeviceAction::Update => "Change faceplate",
        DeviceAction::FactoryReset => "Factory reset",
    }
}

async fn choose_faceplate(
    client: &ResidentClient,
    class: Option<&str>,
) -> Result<FaceplateSelection> {
    let Some(class) = class else {
        return Ok(FaceplateSelection::NoneDeclared);
    };
    let choices = client.faceplates(class).await?;
    if choices.is_empty() {
        return Ok(FaceplateSelection::NoneDeclared);
    }
    println!("\nFaceplates and mount variants:");
    for (index, face) in choices.iter().enumerate() {
        println!(
            "  {}. {} · {}{}{}",
            index + 1,
            face.name,
            face.id,
            face.mount
                .as_deref()
                .map(|m| format!(" · {m}"))
                .unwrap_or_default(),
            face.version
                .as_deref()
                .map(|v| format!(" · v{v}"))
                .unwrap_or_default(),
        );
        if !face.blurb.is_empty() {
            println!("     {}", face.blurb);
        }
    }
    println!("  b. Cancel");
    let Some(answer) = prompt("Choose a faceplate > ")? else {
        return Ok(FaceplateSelection::Cancelled);
    };
    if matches!(answer.trim(), "b" | "back" | "") {
        println!("Faceplate change cancelled.");
        return Ok(FaceplateSelection::Cancelled);
    }
    let index = answer
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=choices.len()).contains(n))
        .ok_or_else(|| anyhow::anyhow!("no such faceplate"))?;
    Ok(FaceplateSelection::Selected(choices[index - 1].id.clone()))
}

async fn follow_maintenance(client: &ResidentClient, device_id: &str) -> Result<()> {
    println!("Following maintenance (Ctrl-C detaches; the Resident keeps working)…");
    let mut stream = client.events().await?;
    let mut shown_steps = 0usize;
    let mut completion_announced = false;
    loop {
        let event = stream.next_json().await?;
        let individuals = individuals_from_event(&event)?;
        let Some(individual) = individuals.iter().find(|i| i.device_id == device_id) else {
            continue;
        };
        if let Some(saga) = &individual.maintenance {
            for step in saga.steps.iter().skip(shown_steps) {
                println!(
                    "  [{}/{}] {}{}{}",
                    step.index,
                    step.total,
                    step.name,
                    if step.ok { "" } else { " — FAILED" },
                    if step.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", step.detail)
                    },
                );
            }
            shown_steps = shown_steps.max(saga.steps.len());
            if saga.state == "failed" {
                bail!(
                    "the {} saga failed — inspect `journalctl -u suzu@<keeper>`",
                    saga.kind
                );
            }
            if saga.state == "done" && !completion_announced {
                println!("  maintenance complete — admission is deciding the stream…");
                completion_announced = true;
            }
        }
        if completion_announced && individual.lifecycle == Lifecycle::Live {
            println!("  LIVE — admission passed and the stream is flowing.");
            return Ok(());
        }
    }
}

fn individuals_from_event(event: &Value) -> Result<Vec<Individual>> {
    match event.get("type").and_then(Value::as_str) {
        Some("snapshot") => serde_json::from_value(
            event
                .pointer("/snapshot/roster")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .context("invalid roster in snapshot"),
        Some("roster") => serde_json::from_value(
            event
                .get("individuals")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .context("invalid roster event"),
        _ => Ok(Vec::new()),
    }
}

fn confirm(question: &str) -> Result<bool> {
    Ok(prompt(&format!("{question} [y/N] > "))?
        .is_some_and(|answer| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")))
}

fn prompt(text: &str) -> Result<Option<String>> {
    print!("{text}");
    io::stdout().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Ok(None);
    }
    Ok(Some(answer.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_ignores_ping_and_reads_the_shared_fact() {
        let mut bytes =
            b": ping\n\nevent: fact\ndata: {\"type\":\"roster\",\"individuals\":[]}\n\n".to_vec();
        assert!(take_sse_json(&mut bytes).unwrap().is_none());
        assert_eq!(
            take_sse_json(&mut bytes).unwrap().unwrap()["type"],
            "roster"
        );
    }

    #[test]
    fn an_http_separator_is_found_without_decoding_the_body() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(find_bytes(raw, b"\r\n\r\n"), Some(34));
    }
}
