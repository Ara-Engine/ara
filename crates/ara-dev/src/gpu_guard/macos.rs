//! Unprivileged Apple process CPU counters and AGX per-client cumulative GPU time.
//! IORegistry counters are observations, never inferred from memory or total GPU load.
use super::{
    cpu,
    model::{Process, Snapshot},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
    time::Instant,
};

#[repr(C)]
struct Usage {
    uuid: [u8; 16],
    user: u64,
    system: u64,
    idle_wakes: u64,
    interrupt_wakes: u64,
    pageins: u64,
    wired: u64,
    resident: u64,
    footprint: u64,
    start: u64,
    exit: u64,
}

fn processes() -> Result<Vec<cpu::Process>> {
    // SAFETY: null/zero requests a count; no process is altered.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    ensure!(count > 0, "proc_listallpids count failed");
    let mut pids = vec![0_i32; count as usize + 256];
    // SAFETY: the writable slice's exact byte capacity is supplied.
    let used = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    ensure!(
        used > 0 && (used as usize) < pids.len(),
        "process snapshot overflowed; retry sampling"
    );
    pids.truncate(used as usize);
    let mut result = Vec::new();
    for pid in pids.into_iter().filter(|p| *p > 0) {
        // SAFETY: zeroed outputs have the SDK's proc_bsdinfo and rusage_info_v0 layouts.
        let (mut info, mut usage): (libc::proc_bsdinfo, Usage) = unsafe { std::mem::zeroed() };
        // SAFETY: query-only PID flavor 3, exact output pointer and length from libproc.h.
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                std::mem::size_of_val(&info) as i32,
            )
        };
        if n != std::mem::size_of_val(&info) as i32 {
            continue;
        }
        // SAFETY: flavor zero writes rusage_info_v0. Inaccessible/exited tasks stay
        // in the GPU ancestry snapshot with zero CPU; total CPU still charges them.
        let read = unsafe { libc::proc_pid_rusage(pid, 0, (&mut usage as *mut Usage).cast()) };
        let name = info
            .pbi_name
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect::<Vec<_>>();
        let name = if name.is_empty() {
            info.pbi_comm
                .iter()
                .take_while(|c| **c != 0)
                .map(|c| *c as u8)
                .collect()
        } else {
            name
        };
        result.push(cpu::Process {
            pid: pid as u32,
            parent: info.pbi_ppid,
            birth: info.pbi_start_tvsec * 1000 + info.pbi_start_tvusec / 1000,
            name: String::from_utf8_lossy(&name).into_owned(),
            ticks: if read == 0 {
                usage.user.saturating_add(usage.system)
            } else {
                0
            },
        });
    }
    Ok(result)
}

pub(super) fn cpu() -> Result<cpu::Raw> {
    // SAFETY: the host right and POD CPU tick output match host_statistics's SDK ABI.
    let mut ticks: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: obtain one query-only send right, released below on every path.
    let host = unsafe { libc::mach_host_self() };
    // SAFETY: four natural_t counters and their exact count are writable.
    let status = unsafe {
        libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO,
            (&mut ticks as *mut libc::host_cpu_load_info).cast(),
            &mut count,
        )
    };
    // SAFETY: release exactly the host send right obtained above.
    unsafe { libc::mach_port_deallocate(libc::mach_task_self(), host) };
    ensure!(
        status == 0 && count == libc::HOST_CPU_LOAD_INFO_COUNT,
        "host CPU ticks unavailable: {status}"
    );
    // SAFETY: sysconf reads the system clock tick frequency.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    ensure!(hz > 0, "CPU tick frequency unavailable");
    let ns = |v: u64| v * 1_000_000_000 / hz as u64;
    Ok(cpu::Raw {
        at: super::unix_ms(),
        total: ns(ticks.cpu_ticks.iter().map(|v| u64::from(*v)).sum()),
        idle: ns(u64::from(ticks.cpu_ticks[libc::CPU_STATE_IDLE as usize])),
        processes: processes()?,
    })
}

fn counters() -> Result<BTreeMap<(u64, u32), u64>> {
    let output = Command::new("/usr/sbin/ioreg")
        .args(["-r", "-c", "AGXDeviceUserClient", "-a"])
        .output()?;
    ensure!(output.status.success(), "AGX client counters unavailable");
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child
        .stdin
        .take()
        .context("plutil input")?
        .write_all(&output.stdout)?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "AGX counter plist could not be decoded"
    );
    super::agx::parse(&serde_json::from_slice(&output.stdout)?)
}
pub(super) struct Sampler {
    previous: BTreeMap<(u64, u32), u64>,
    at: Instant,
}
impl Sampler {
    pub fn new() -> Result<Self> {
        Ok(Self {
            previous: counters()?,
            at: Instant::now(),
        })
    }
    pub fn sample(&mut self) -> Result<Snapshot> {
        let current = counters()?;
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64() * 1e9;
        ensure!(elapsed > 0., "AGX sampling window is empty");
        let mut engines = Vec::new();
        for ((id, pid), time) in &current {
            let previous = self.previous.get(&(*id, *pid)).copied().unwrap_or(0);
            ensure!(*time >= previous, "AGX cumulative counter regressed");
            engines.push((
                format!("pid_{pid}_agx_{id}"),
                100. * (*time - previous) as f64 / elapsed,
            ));
        }
        self.previous = current;
        self.at = now;
        Ok(Snapshot {
            unix_ms: super::unix_ms(),
            engines,
            processes: processes()?
                .into_iter()
                .map(|p| Process {
                    pid: p.pid,
                    parent: p.parent,
                    name: p.name,
                    birth: p.birth,
                })
                .collect(),
        })
    }
}
