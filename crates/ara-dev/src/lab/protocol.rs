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
    // Accepted Windows sockets can inherit the listener's nonblocking mode.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
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

fn schema(mut properties: Value, required: &[&str]) -> Value {
    properties["platform"] = json!({"type":"string","enum":["pc","mac"],"default":"pc"});
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn tools() -> Value {
    let string = json!({"type":"string"});
    let session = json!({"type":"string","default":"default"});
    let definitions = [
        (
            "lab_mirror",
            "Configure the PC-owned Mac mirror; AC power is required before remote jobs.",
            schema(
                json!({"host":string,"repo":string,"build_dir":string,"pc_repository":string}),
                &[],
            ),
        ),
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
                json!({"session":session,"close":{"type":"boolean"},"scene":{"type":"string","enum":["showcase","outpost","hangar"]},"pack":string,"preset":string,"width":{"type":"integer","minimum":16},"height":{"type":"integer","minimum":16}}),
                &[],
            ),
        ),
        (
            "lab_capture",
            "E0 fixed-camera captures: matched EXR display transform, region gates, three-run repeatability and static/pan block RMS; LDR FLIP/SSIM are secondary. runs:1,temporal:false selects a quick sweep.",
            schema(
                json!({"session":session,"runs":{"type":"integer","minimum":1,"maximum":3,"default":3},"temporal":{"type":"boolean","default":true}}),
                &[],
            ),
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
            "lab_variant",
            "Register/select an isolated offline PSO variant. New recipes cook before automatic selection.",
            schema(
                json!({"session":session,"name":string,"promote":{"type":"boolean"},"remove":{"type":"boolean"},"recipe":{"type":"object","properties":{"packages":{"type":"object"}},"required":["packages"]}}),
                &["name"],
            ),
        ),
        (
            "lab_ab",
            "Counterbalanced baseline A versus experimental B in one warm process, under one guarded GPU lease; returns paired deltas and noise.",
            schema(
                json!({"session":session,"a":{"type":"string","enum":["baseline"]},"b":string,"windows":{"type":"integer","minimum":2,"maximum":100},"frames":{"type":"integer","minimum":1,"maximum":100000},"preset":{"type":"string","enum":["low","medium","high","epic"]},"no_wait":{"type":"boolean"},"guard":{"type":"object"}}),
                &["b"],
            ),
        ),
        (
            "lab_set",
            "Discover typed dev tunables, or atomically push name -> {type:f32|int|bool|vec,value} updates. Types and ranges are validated before one uniform upload.",
            schema(
                json!({"session":session,"camera":string,"values":{"type":"object"}}),
                &[],
            ),
        ),
        (
            "lab_reference",
            "Render optional Blender 5.1 Cycles EXR references under target; missing scene sources/tools are diagnosed.",
            schema(
                json!({"scene":{"type":"string","enum":["showcase","outpost","hangar"]},"blender":string,"bench_root":string,"samples":{"type":"integer","minimum":1,"maximum":8192}}),
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

#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    fn accepted_clients_wait_for_the_first_payload() -> Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        let (stream, _) = listener.accept()?;
        stream.set_nonblocking(true)?;
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let _ = ready.send(());
            read(&stream)
        });
        waiting.recv()?;
        std::thread::sleep(Duration::from_millis(10));
        writeln!(
            client,
            "{{\"token\":\"test\",\"tool\":\"lab_status\",\"arguments\":{{}}}}"
        )?;
        let request = reader
            .join()
            .map_err(|_| anyhow::anyhow!("reader panicked"))??;
        ensure!(request.tool == "lab_status", "request changed");
        Ok(())
    }
}
