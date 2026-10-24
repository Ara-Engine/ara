//! Engine management is global, without an engine session or an open project.
use ara_mcp::GlobalAdapter;
use ara_proto::{Endpoint, RpcError};
use serde_json::{Value, json};

struct FakeHub;
impl Endpoint for FakeHub {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "hub.catalog" {
            return Ok(json!({"methods":ara_host::core_methods()}));
        }
        Ok(json!({"method":method,"params":params}))
    }
}
#[test]
fn engine_actions_share_the_hub_install_job_api() {
    let mut adapter = GlobalAdapter::new();
    let mut hub = FakeHub;
    for (action, method, args) in [
        ("engines", "hub.engines", json!({})),
        ("checkReleases", "hub.releases.check", json!({})),
        (
            "available",
            "hub.engines.available",
            json!({"channel":"preview"}),
        ),
        (
            "installEngine",
            "hub.engine.install",
            json!({"version":"0.0.1-alpha"}),
        ),
        (
            "uninstallEngine",
            "hub.engine.uninstall",
            json!({"version":"0.0.1-alpha"}),
        ),
        ("downloads", "hub.downloads", json!({})),
        (
            "cancelDownload",
            "hub.download.cancel",
            json!({"job":"engine-0.0.1-alpha"}),
        ),
    ] {
        let mut arguments = args.clone();
        arguments["action"] = json!(action);
        let reply=adapter.handle(&mut hub,json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ara_projects","arguments":arguments}})).expect("reply");
        assert_eq!(reply["result"]["structuredContent"]["method"], method);
        assert_eq!(reply["result"]["structuredContent"]["params"], args);
    }
    let list = adapter
        .handle(
            &mut hub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )
        .expect("tools");
    let projects = &list["result"]["tools"][0];
    assert_eq!(projects["name"], "ara_projects");
    assert!(
        projects["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|a| a == "installEngine")
    );
}
