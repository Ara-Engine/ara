//! A small stdio MCP bridge to a warm player's explicit local control queue.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn enqueue(session: &Path, request: &Value) -> Result<()> {
    ensure!(
        session.is_dir(),
        "perf control directory does not exist: {}",
        session.display()
    );
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temp = session.join(format!("{id}.tmp"));
    fs::write(&temp, serde_json::to_vec_pretty(request)?)?;
    fs::rename(temp, session.join(format!("{id}.json")))?;
    Ok(())
}

fn dispatch(session: &Path, method: &str, params: &Value) -> Result<Value> {
    Ok(match method {
        "initialize" => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"ara-dev-perf","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":[
            {"name":"perf_status","description":"Read the last completed frame window, GPU pass timings and shader reload provenance.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"perf_measure","description":"Measure another window in the existing warm player.","inputSchema":{"type":"object","properties":{"quality":{"type":"string","enum":["low","medium","high","epic"]}},"additionalProperties":false}},
            {"name":"perf_reload_shaders","description":"Reload successfully cooked shaders asynchronously, retain last good on errors, then measure.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"perf_stop","description":"Stop this perf player and release its machine GPU lock.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}
        ]}),
        "tools/call" => {
            let name = params["name"].as_str().context("tool name")?;
            let value = if name == "perf_status" {
                let directory = session.parent().context("session parent")?;
                let read = |name: &str| -> Result<Value> {
                    Ok(serde_json::from_slice(&fs::read(directory.join(name))?)?)
                };
                json!({"live":read("status.json").ok(),"latest_measurement":read("player.json").ok()})
            } else {
                let quality = params["arguments"]["quality"].as_str();
                ensure!(
                    quality.is_none_or(|q| ["low", "medium", "high", "epic"].contains(&q)),
                    "unknown quality"
                );
                let request = match name {
                    "perf_measure" => json!({"measure":true,"quality":quality}),
                    "perf_reload_shaders" => json!({"measure":true,"reload_shaders":true}),
                    "perf_stop" => json!({"stop":true}),
                    _ => bail!("unknown tool {name}"),
                };
                enqueue(session, &request)?;
                json!({"queued":true})
            };
            json!({"content":[{"type":"text","text":serde_json::to_string(&value)?}],"structuredContent":value,"isError":false})
        }
        _ => bail!("unknown MCP method {method}"),
    })
}

pub(super) fn mcp(session: &Path) -> Result<bool> {
    ensure!(session.is_dir(), "perf session does not exist");
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = dispatch(
            session,
            request["method"].as_str().context("method")?,
            &request["params"],
        );
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("{error:#}")}})
            }
        };
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mcp_lists_only_local_perf_tools_and_rejects_unknown_operations() {
        let session = Path::new("missing-session");
        let tools = dispatch(session, "tools/list", &Value::Null).unwrap();
        assert_eq!(tools["tools"].as_array().unwrap().len(), 4);
        assert!(dispatch(session, "tools/call", &json!({"name":"unknown"})).is_err());
        assert!(
            dispatch(
                session,
                "tools/call",
                &json!({"name":"perf_measure","arguments":{"quality":"auto"}})
            )
            .is_err()
        );
    }
}
