//! Native PDH queries avoid launching a PowerShell process every second.
use super::model::{Process, Snapshot};
use anyhow::{Result, bail, ensure};
use std::{
    collections::BTreeMap,
    mem, ptr,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Performance::*,
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

pub(super) struct Sampler {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}
fn check(code: u32, operation: &str) -> Result<()> {
    ensure!(code == 0, "{operation}: PDH 0x{code:08x}");
    Ok(())
}
impl Sampler {
    pub fn new() -> Result<Self> {
        let mut query = ptr::null_mut();
        check(
            // SAFETY: null selects live counters; output points to a query handle.
            unsafe { PdhOpenQueryW(ptr::null(), 0, &mut query) },
            "open GPU query",
        )?;
        let mut sampler = Self {
            query,
            counter: ptr::null_mut(),
        };
        let path: Vec<_> = r"\GPU Engine(*)\Utilization Percentage"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        check(
            // SAFETY: query is live, path terminated, output counter writable.
            unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut sampler.counter) },
            "add per-process GPU counter",
        )?;
        // SAFETY: prime the live query; percentages require two collections.
        let code = unsafe { PdhCollectQueryData(query) };
        if code != PDH_NO_DATA {
            check(code, "prime GPU counters")?;
        }
        Ok(sampler)
    }
    pub fn sample(&mut self) -> Result<Snapshot> {
        // SAFETY: query belongs to this sampler and remains open.
        let code = unsafe { PdhCollectQueryData(self.query) };
        let engines = if code == PDH_NO_DATA {
            Vec::new()
        } else {
            check(code, "collect GPU counters")?;
            self.values()?
        };
        let processes = processes()?;
        Ok(Snapshot {
            unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
            processes,
            engines,
        })
    }
    fn values(&self) -> Result<Vec<(String, f64)>> {
        for _ in 0..3 {
            let (mut bytes, mut count) = (0, 0);
            // SAFETY: zero-buffer size probe writes required bytes and item count.
            let code = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    ptr::null_mut(),
                )
            };
            if code == PDH_NO_DATA || code == PDH_CSTATUS_NO_INSTANCE {
                return Ok(Vec::new());
            }
            ensure!(
                code == PDH_MORE_DATA || code == 0,
                "size GPU counters: PDH 0x{code:08x}"
            );
            if bytes == 0 {
                return Ok(Vec::new());
            }
            // usize storage supplies sufficient alignment for the Win64 structures.
            let mut storage = vec![0usize; (bytes as usize).div_ceil(mem::size_of::<usize>())];
            let capacity = storage.len() * mem::size_of::<usize>();
            let output = storage.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            // SAFETY: storage is aligned and contains at least the requested bytes.
            let code = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    output,
                )
            };
            if code == PDH_MORE_DATA {
                continue;
            }
            check(code, "read GPU counters")?;
            ensure!(
                (count as usize) * mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() <= capacity,
                "GPU counter array exceeds allocation"
            );
            // SAFETY: PDH initialized count items; bounds and alignment are checked.
            let items = unsafe { std::slice::from_raw_parts(output, count as usize) };
            let mut values = Vec::new();
            for item in items {
                if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA
                    && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA
                {
                    continue;
                }
                let offset = (item.szName as usize)
                    .checked_sub(storage.as_ptr() as usize)
                    .filter(|o| *o < capacity && o % 2 == 0)
                    .ok_or_else(|| anyhow::anyhow!("GPU instance name outside PDH storage"))?;
                // SAFETY: the name points inside the live buffer, bounded to its end.
                let name =
                    unsafe { std::slice::from_raw_parts(item.szName, (capacity - offset) / 2) };
                let end = name
                    .iter()
                    .position(|c| *c == 0)
                    .ok_or_else(|| anyhow::anyhow!("unterminated GPU instance name"))?;
                // SAFETY: PDH_FMT_DOUBLE requested the initialized union member.
                values.push((String::from_utf16_lossy(&name[..end]), unsafe {
                    item.FmtValue.Anonymous.doubleValue
                }));
            }
            return Ok(values);
        }
        bail!("GPU counter instances changed repeatedly while reading")
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        // SAFETY: this owner closes the query and its counters once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

fn processes() -> Result<Vec<Process>> {
    // SAFETY: process-only enumeration, no remote process mutation.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    ensure!(
        snapshot != INVALID_HANDLE_VALUE,
        "enumerate GPU process identities: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: PROCESSENTRY32W is POD; dwSize is set before the API call.
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of_val(&entry) as u32;
    let mut result = BTreeMap::new();
    // SAFETY: snapshot and sized output entry are valid.
    let mut found = unsafe { Process32FirstW(snapshot, &mut entry) };
    while found != 0 {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        result.insert(
            entry.th32ProcessID,
            Process {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..end]),
                birth: birth(entry.th32ProcessID),
            },
        );
        // SAFETY: same live snapshot and output entry.
        found = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: snapshot is no longer used.
    unsafe { CloseHandle(snapshot) };
    Ok(result.into_values().collect())
}
fn birth(pid: u32) -> u64 {
    // SAFETY: request read-only, limited process metadata.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return 0;
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: live read-only handle and four writable FILETIME values.
    let read =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: read-only handle has one owner.
    unsafe { CloseHandle(process) };
    if read == 0 {
        0
    } else {
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    }
}
