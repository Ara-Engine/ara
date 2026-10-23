use super::*;
use ara_proto::Endpoint;
use serde_json::json;
use std::io::{Read,Write};
#[test]
fn websocket_edits_and_mcp_capture_share_the_window_authority(){
    let root=std::env::temp_dir().join(format!("ara-editor-wire-{}",std::process::id()));std::fs::create_dir(&root).unwrap();std::fs::create_dir(root.join("levels")).unwrap();
    std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"name":"Wire test"}))).unwrap();std::fs::write(root.join("levels/main.scene"),ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_wire0001","records":{"e_crate001":{"name":"Crate","Transform":{}}}}))).unwrap();
    let mut editor=Editor::open(&root).unwrap();let mut server=Server::start(&root).unwrap();let client_root=root.clone();let(tx,rx)=mpsc::channel();
    std::thread::spawn(move||{
        let mut client=ara_rpc::Client::for_project(&client_root).unwrap();let session=client.call("session.begin",json!({"client":"wire-test","author":"agent:wire"})).unwrap()["session"].as_str().unwrap().to_owned();
        let applied=client.call("cmd.apply",json!({"session":session,"doc":"sc_wire0001","why":"Network edit","commands":[{"cmd":"transform.set","args":{"entities":["e_crate001"],"position":[4,0,0]}}]})).unwrap();
        let info=ara_rpc::DaemonInfo::read(&client_root).unwrap();let body=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ara_capture","arguments":{"session":session,"target":"layout"}}}).to_string();let mut stream=std::net::TcpStream::connect(("127.0.0.1",info.port)).unwrap();stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();write!(stream,"POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",info.token,body.len(),body).unwrap();let mut response=String::new();stream.read_to_string(&mut response).unwrap();let(_,body)=response.split_once("\r\n\r\n").unwrap();let capture:serde_json::Value=serde_json::from_str(body).unwrap();tx.send((applied,capture)).unwrap();
    });
    let deadline=Instant::now()+Duration::from_secs(10);let result=loop{server.pump(&mut editor);editor.tick();if let Ok(result)=rx.try_recv(){break result;}assert!(Instant::now()<deadline,"network test timed out");std::thread::sleep(Duration::from_millis(1));};
    assert_ne!(result.1["result"]["isError"],true,"{}",result.1);let job=result.1["result"]["structuredContent"]["job"].as_str().unwrap();let task=editor.captures.try_recv().unwrap();assert_eq!(task.job,job);assert_eq!(task.params.target.as_deref(),Some("layout"));
    assert_eq!(editor.host.store().workspace().doc("sc_wire0001").unwrap().record("e_crate001").unwrap()["Transform"]["position"],json!([4.0,0.0,0.0]));editor.action("review.revert",json!({"txn":result.0["txn"]})).unwrap();
    drop(server);drop(editor);assert!(ara_rpc::DaemonInfo::read(&root).is_none());assert!(root.starts_with(std::env::temp_dir()));std::fs::remove_dir_all(root).unwrap();
}
use std::time::Instant;
