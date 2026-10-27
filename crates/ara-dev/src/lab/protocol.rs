//! Loopback transport with a checkout token; stdio remains a thin MCP client.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    time::Duration,
};

#[derive(Serialize, Deserialize)]
pub(super) struct Endpoint {
    pub address: std::net::SocketAddr,
    pub token: String,
    pub pid: u32,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    pub token: String,
    pub tool: String,
    pub arguments: Value,
}

pub(super) fn read(stream: &TcpStream) -> Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(1_048_576)
        .read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}
pub(super) fn reply(mut stream: TcpStream, result: Result<Value>) -> Result<()> {
    let value = match result {
        Ok(value) => json!({"result":value}),
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    writeln!(stream, "{value}")?;
    Ok(())
}
pub(super) fn call(root: &Path, tool: &str, arguments: Value) -> Result<Value> {
    let endpoint: Endpoint = serde_json::from_slice(
        &fs::read(super::directory(root).join("endpoint.json"))
            .context("lab is not running; run ara-dev lab start")?,
    )?;
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&Request {
            token: endpoint.token,
            tool: tool.into(),
            arguments
        })?
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line)?;
    if let Some(error) = reply["error"].as_str() {
        bail!("{error}");
    }
    Ok(reply["result"].clone())
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn tools() -> Value {
    let string = json!({"type":"string"});
    let session = json!({"type":"string","default":"default"});
    let definitions = [
        (
            "lab_status",
            "Builds, claims, shader errors and resident tracks.",
            schema(json!({}), &[]),
        ),
        (
            "lab_claim",
            "Declare workspace-relative files/directories; overlapping ownership warns.",
            schema(
                json!({"agent":string,"paths":{"type":"array","items":string,"minItems":1}}),
                &["agent", "paths"],
            ),
        ),
        (
            "lab_release",
            "Release an agent's claims.",
            schema(json!({"agent":string}), &["agent"]),
        ),
        (
            "lab_reload",
            "Reload successfully cooked shaders; the watcher cooks source changes automatically.",
            schema(json!({"session":session}), &[]),
        ),
        (
            "lab_rebuild",
            "Queue an incremental Rust rebuild and state-preserving warm restart.",
            schema(json!({}), &[]),
        ),
        (
            "lab_session",
            "Select a frozen scene and create a hidden warm track.",
            schema(
                json!({"session":session,"scene":{"type":"string","enum":["showcase","outpost","hangar"]},"pack":string,"preset":string,"width":{"type":"integer","minimum":16},"height":{"type":"integer","minimum":16}}),
                &[],
            ),
        ),
        (
            "lab_capture",
            "Capture fixed cameras, reference FLIP/SSIM, regions and crops.",
            schema(json!({"session":session}), &[]),
        ),
        (
            "lab_measure",
            "Measure an exclusive GPU-queue window in the same player.",
            schema(
                json!({"session":session,"preset":{"type":"string","enum":["low","medium","high","epic"]},"frames":{"type":"integer","minimum":1,"maximum":100000},"no_wait":{"type":"boolean"},"guard":{"type":"object","properties":{"cpu_threshold_percent":{"type":"number","minimum":0,"maximum":100},"cpu_sustained_samples":{"type":"integer","minimum":1},"threshold_percent":{"type":"number","minimum":0},"sustained_samples":{"type":"integer","minimum":1},"dwm_allowance_percent":{"type":"number","minimum":0}},"additionalProperties":false}}),
                &[],
            ),
        ),
        (
            "lab_logs",
            "Recent daemon events and owner-attributed errors.",
            schema(
                json!({"lines":{"type":"integer","minimum":1,"maximum":1000}}),
                &[],
            ),
        ),
        (
            "lab_stop",
            "Stop the daemon and only its owned players.",
            schema(json!({}), &[]),
        ),
    ];
    json!({"tools":definitions.into_iter().map(|(name,description,input_schema)| json!({"name":name,"description":description,"inputSchema":input_schema})).collect::<Vec<_>>()})
}
pub(super) fn mcp(root: &Path) -> Result<bool> {
    let mut output = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str() {
            Some("initialize") => Ok(
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"ara-fidelity-lab","version":env!("CARGO_PKG_VERSION")}}),
            ),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(tools()),
            Some("tools/call") => (|| {
                let name = request["params"]["name"].as_str().context("tool name")?;
                ensure!(name.starts_with("lab_"), "unknown lab tool");
                let result = call(root, name, request["params"]["arguments"].clone());
                Ok(match result {
                    Ok(value) => {
                        json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
                    }
                    Err(error) => {
                        json!({"content":[{"type":"text","text":format!("{error:#}")}],"isError":true})
                    }
                })
            })(),
            _ => Err(anyhow::anyhow!("unsupported MCP method")),
        };
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error.to_string()}})
            }
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(true)
}
