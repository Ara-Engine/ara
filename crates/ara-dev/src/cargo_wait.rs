//! Exclude reported Cargo package-cache waits from the gate's execution budget.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::process::{self, Outcome};

struct Clock {
    limit: Option<Instant>,
    paused: Option<Instant>,
    waited: Duration,
}

impl Clock {
    fn new(limit: Option<Instant>) -> Self {
        Self {
            limit,
            paused: None,
            waited: Duration::ZERO,
        }
    }

    fn line(&mut self, line: &str, now: Instant) -> Result<()> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let cache_wait = line.strip_prefix("Blocking").is_some_and(|message| {
            message
                .trim_start()
                .starts_with("waiting for file lock on ")
                && message.contains("package cache")
        });
        if cache_wait {
            self.paused.get_or_insert(now);
        } else {
            self.resume(now)?;
        }
        Ok(())
    }

    fn resume(&mut self, now: Instant) -> Result<()> {
        if let Some(start) = self.paused.take() {
            self.waited = self
                .waited
                .checked_add(now.saturating_duration_since(start))
                .context("Cargo cache wait is too large")?;
        }
        Ok(())
    }

    fn deadline(&self, now: Instant) -> Result<Option<Instant>> {
        let active = self
            .paused
            .map_or(Duration::ZERO, |start| now.saturating_duration_since(start));
        let waited = self
            .waited
            .checked_add(active)
            .context("Cargo cache wait is too large")?;
        self.limit
            .map(|limit| {
                limit
                    .checked_add(waited)
                    .context("gate deadline is too large")
            })
            .transpose()
    }
}

struct Tail {
    file: File,
    pending: Vec<u8>,
}

impl Tail {
    fn open(path: &Path) -> Result<Self> {
        // A separate handle keeps reads from moving the child's write cursor.
        Ok(Self {
            file: File::open(path)?,
            pending: Vec::new(),
        })
    }

    fn poll(&mut self, clock: &mut Clock, now: Instant) -> Result<()> {
        self.file.read_to_end(&mut self.pending)?;
        let mut consumed = 0;
        for (i, &byte) in self.pending.iter().enumerate() {
            if byte == b'\n' {
                clock.line(&String::from_utf8_lossy(&self.pending[consumed..i]), now)?;
                consumed = i + 1;
            }
        }
        self.pending.drain(..consumed);
        Ok(())
    }
}

/// Further Cargo output ends a reported wait. Keep wall times in the report, but
/// carry the excluded interval forward to every later phase's gate deadline.
pub(crate) fn run(
    command: &mut Command,
    deadline: &mut Option<Instant>,
    stdout: &Path,
    stderr: &Path,
) -> Result<(Outcome, Duration)> {
    let mut tails = vec![Tail::open(stdout)?];
    if stdout != stderr {
        tails.push(Tail::open(stderr)?);
    }
    let mut clock = Clock::new(*deadline);
    let outcome = process::run_with_deadline(command, || {
        let now = Instant::now();
        for tail in &mut tails {
            tail.poll(&mut clock, now)?;
        }
        clock.deadline(now)
    });
    let now = Instant::now();
    for tail in &mut tails {
        tail.poll(&mut clock, now)?;
    }
    clock.resume(now)?;
    *deadline = clock.deadline(now)?;
    Ok((outcome?, clock.waited))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_waits_pause_the_budget_and_progress_resumes_it() {
        let now = Instant::now();
        let mut clock = Clock::new(Some(now + Duration::from_secs(10)));
        clock
            .line(
                " Blocking waiting for file lock on package cache",
                now + Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(
            clock.deadline(now + Duration::from_secs(103)).unwrap(),
            Some(now + Duration::from_secs(110))
        );
        clock
            .line("   Doc-tests ara_host", now + Duration::from_secs(103))
            .unwrap();
        assert_eq!(
            clock.deadline(now + Duration::from_secs(104)).unwrap(),
            Some(now + Duration::from_secs(110))
        );
        clock
            .line(
                "Blocking waiting for file lock on shared package cache",
                now + Duration::from_secs(105),
            )
            .unwrap();
        clock
            .line(
                r#"{"reason":"compiler-artifact","fresh":true}"#,
                now + Duration::from_secs(125),
            )
            .unwrap();
        assert_eq!(
            clock.deadline(now + Duration::from_secs(129)).unwrap(),
            Some(now + Duration::from_secs(130))
        );
        assert_eq!(clock.waited, Duration::from_secs(120));
    }

    #[test]
    fn build_directory_waits_do_not_disable_the_execution_deadline() {
        let now = Instant::now();
        let mut clock = Clock::new(Some(now + Duration::from_secs(10)));
        clock
            .line("Blocking waiting for file lock on build directory", now)
            .unwrap();
        assert_eq!(
            clock.deadline(now + Duration::from_secs(20)).unwrap(),
            clock.limit
        );
        let mut unlimited = Clock::new(None);
        unlimited
            .line(
                "Blocking waiting for file lock on package cache (build)",
                now,
            )
            .unwrap();
        assert_eq!(
            unlimited.deadline(now + Duration::from_secs(3600)).unwrap(),
            None
        );
    }

    #[test]
    fn appended_split_lines_are_observed_without_moving_the_writer() {
        use std::io::Write;
        let directory = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .join("ara-dev");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("cargo-wait-{}.log", std::process::id()));
        let mut writer = File::create(&path).unwrap();
        let mut tail = Tail::open(&path).unwrap();
        let now = Instant::now();
        let mut clock = Clock::new(Some(now + Duration::from_secs(10)));
        write!(writer, "Block").unwrap();
        tail.poll(&mut clock, now).unwrap();
        assert!(clock.paused.is_none());
        writeln!(writer, "ing waiting for file lock on package cache").unwrap();
        tail.poll(&mut clock, now + Duration::from_secs(1)).unwrap();
        assert!(clock.paused.is_some());
        writeln!(writer, "   Doc-tests fixture").unwrap();
        tail.poll(&mut clock, now + Duration::from_secs(4)).unwrap();
        assert!(clock.paused.is_none());
        assert_eq!(clock.waited, Duration::from_secs(3));
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .starts_with("Blocking ")
        );
    }
}
