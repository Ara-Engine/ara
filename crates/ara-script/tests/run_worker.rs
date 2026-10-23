//! The `ara_run` worker: typed engine calls through a sink, the IL whitelist, compile
//! and command errors, and wall-clock and memory limits that kill the worker (a warm
//! spare takes over).

mod support;

use std::time::{Duration, Instant};

use ara_diag::Diagnostic;
use ara_script::codes;
use ara_script::run::{CommandSink, RunLimits, RunPool};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    created: u32,
}

impl CommandSink for Fake {
    fn call(&mut self, command: &str, params: &Value) -> Result<Value, Diagnostic> {
        match command {
            "entity.create" => {
                self.created += 1;
                Ok(json!({ "id": format!("e_{}", self.created), "name": params["name"] }))
            }
            _ => Err(Diagnostic::error(ara_diag::codes::UNKNOWN_COMMAND, format!("unknown command `{command}`"))),
        }
    }
}

fn first(errors: &[Diagnostic]) -> &Diagnostic {
    errors.first().expect("a diagnostic")
}

#[test]
fn scripts_use_typed_calls_and_are_killed_on_limits() {
    support::build();
    let worker = support::bin("Ara.RunWorker").join("Ara.RunWorker.dll");
    let mut pool = RunPool::new(&worker, 768 << 20).expect("worker pool");
    let limits = RunLimits { wall: Duration::from_secs(30), memory: 768 << 20 };
    let mut sink = Fake::default();

    let out = pool
        .run(
            r#"var a = await ara.Call("entity.create", new { name = "Crate" });
await ara.Call("entity.create", new { name = "Barrel" });
ara.Why = "stock the room";
ara.Log("made two");
return new { created = 2, first = (string?)a?["id"] };"#,
            limits,
            &mut sink,
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(out.result, json!({ "created": 2, "first": "e_1" }));
    assert_eq!(out.calls, vec![("entity.create".into(), json!({ "name": "Crate" })), ("entity.create".into(), json!({ "name": "Barrel" }))]);
    assert_eq!((out.why.as_deref(), out.log.as_slice()), (Some("stock the room"), &["made two".to_owned()][..]));

    // Ambient authority is not on the whitelist; the worker survives the refusal.
    let err = pool.run(r#"return System.IO.File.ReadAllText("secrets.txt");"#, limits, &mut sink).expect_err("I/O");
    assert_eq!(first(&err).code, codes::RUN_ACCESS, "{err:#?}");
    assert!(first(&err).message.contains("System.IO.File"));
    let err = pool.run("var t = typeof(string).GetMethod(\"Trim\", System.Type.EmptyTypes); return 1;", limits, &mut sink).expect_err("reflection");
    assert!(err.iter().any(|d| d.code == codes::RUN_ACCESS && d.message.contains("GetMethod")), "{err:#?}");

    // Compile errors point at the script line; failed engine calls throw into the script.
    let err = pool.run("var x = 1;\nreturn y;", limits, &mut sink).expect_err("compile error");
    assert_eq!((first(&err).code, first(&err).location.line), (codes::RUN_FAILED, Some(2)), "{err:#?}");
    let err = pool.run("await ara.Call(\"no.such\");\nreturn 0;", limits, &mut sink).expect_err("command error");
    assert!(first(&err).message.contains("ARA1601"), "{err:#?}");

    // A runaway loop is killed at the wall-clock limit; the spare runs the next script.
    let started = Instant::now();
    let err = pool.run("while (true) { }", RunLimits { wall: Duration::from_millis(1500), ..limits }, &mut sink).expect_err("hang");
    assert_eq!(first(&err).code, codes::RUN_LIMIT, "{err:#?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let out = pool.run("return 41 + 1;", limits, &mut sink).expect("the spare takes over");
    assert_eq!(out.result, json!(42));

    // Unbounded allocation hits the memory limit; the worker is killed and replaced.
    let err = pool
        .run("var keep = new List<byte[]>(); while (true) { var b = new byte[1 << 20]; b[0] = 1; keep.Add(b); }", limits, &mut sink)
        .expect_err("memory");
    assert_eq!(first(&err).code, codes::RUN_LIMIT, "{err:#?}");
    assert_eq!(pool.run("return \"ok\";", limits, &mut sink).expect("replacement").result, json!("ok"));
}
