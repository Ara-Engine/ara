//! Resolve pending preparation through the same job protocol for local and remote hosts.
use super::*;

const STALL_ENV: &str = "ARA_JOB_STALL_TIMEOUT_SECS";

pub(super) fn timeout() -> Result<Duration, RpcError> {
    match std::env::var(STALL_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .ok_or_else(|| {
                RpcError::from_diagnostics(vec![Diagnostic::error(
                    codes::INVALID_PARAMS,
                    format!("{STALL_ENV} must be a positive number of seconds"),
                )])
            }),
        Err(std::env::VarError::NotPresent) => Ok(Duration::from_secs(120)),
        Err(error) => Err(RpcError::from_diagnostics(vec![Diagnostic::error(
            codes::INVALID_PARAMS,
            error.to_string(),
        )])),
    }
}

pub(super) fn finish(
    ep: &mut dyn Endpoint,
    session: &str,
    mut result: Value,
    timeout: Duration,
    mut elapsed: impl FnMut() -> Duration,
) -> Result<Value, RpcError> {
    while result["preparing"] == true {
        let Some(job) = result["job"].as_str().map(str::to_owned) else {
            break;
        };
        let mut deadline = elapsed().saturating_add(timeout);
        let mut previous: Option<(Option<f64>, String)> = None;
        loop {
            ep.tick();
            let status = ep.call("jobs.status", json!({"session":session,"job":job}))?;
            let now = elapsed();
            let progress = status["progress"].as_f64();
            let message = status["message"].as_str().unwrap_or("");
            if previous
                .as_ref()
                .is_none_or(|(fraction, phase)| *fraction != progress || phase != message)
            {
                previous = Some((progress, message.into()));
                deadline = now.saturating_add(timeout);
            }
            match status["status"].as_str() {
                Some("done") => {
                    result = status["result"].clone();
                    break;
                }
                Some("failed" | "cancelled") => {
                    let diagnostics =
                        serde_json::from_value::<Vec<Diagnostic>>(status["diagnostics"].clone())
                            .ok()
                            .filter(|d| !d.is_empty())
                            .unwrap_or_else(|| {
                                vec![Diagnostic::error(
                                    codes::IO,
                                    format!("preparation job `{job}` was cancelled"),
                                )]
                            });
                    return Err(RpcError::from_diagnostics(diagnostics));
                }
                _ if now >= deadline => {
                    return Err(RpcError::from_diagnostics(vec![Diagnostic::error(
                        codes::JOB_STALLED,
                        format!(
                            "preparation job `{job}` made no progress for {} seconds (last phase: `{message}`); set {STALL_ENV} to increase the stall budget",
                            timeout.as_secs()
                        ),
                    )]));
                }
                _ => thread::sleep(Duration::from_millis(5)),
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Job {
        poll: u32,
        advances: bool,
        phases: bool,
    }
    impl Endpoint for Job {
        fn call(&mut self, method: &str, _: Value) -> Result<Value, RpcError> {
            assert_eq!(method, "jobs.status");
            self.poll += 1;
            Ok(if (self.advances || self.phases) && self.poll == 30 {
                json!({"status":"done","result":{"ok":true,"txn":"tx_completed"}})
            } else {
                json!({"status":"running", "progress":if self.advances { self.poll as f64 / 30.0 } else { 0.0 },
                    "message":if self.phases { format!("phase {}", self.poll) } else { "cooking".into() }, "heartbeat":self.poll})
            })
        }
    }
    #[test]
    fn progressing_preparation_can_outlast_the_total_wait_budget() {
        let mut elapsed = Duration::ZERO;
        let result = finish(
            &mut Job {
                poll: 0,
                advances: true,
                phases: false,
            },
            "s_test",
            json!({"preparing":true,"job":"jb_test"}),
            Duration::from_secs(120),
            || {
                elapsed += Duration::from_secs(10);
                elapsed
            },
        )
        .expect("300 seconds of advancing work must not hit an absolute 120 second timeout");
        assert_eq!(result["txn"], "tx_completed");
    }

    #[test]
    fn phase_updates_keep_a_long_cook_alive_but_unrelated_heartbeats_do_not() {
        for phases in [false, true] {
            let mut elapsed = Duration::ZERO;
            let result = finish(
                &mut Job {
                    poll: 0,
                    advances: false,
                    phases,
                },
                "s_test",
                json!({"preparing":true,"job":"jb_test"}),
                Duration::from_secs(120),
                || {
                    elapsed += Duration::from_secs(10);
                    elapsed
                },
            );
            if phases {
                assert_eq!(result.expect("phase progress")["txn"], "tx_completed");
            } else {
                let error =
                    result.expect_err("a stalled worker cannot wait forever on heartbeats alone");
                assert_eq!(error.diagnostics()[0].code, codes::JOB_STALLED);
                assert!(error.to_string().contains("jb_test"));
                assert!(error.to_string().contains(STALL_ENV));
            }
        }
    }
}
