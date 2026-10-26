//! Native PDH queries avoid launching a PowerShell process every second.
use super::model::{Process, Snapshot};
use anyhow::{Result, ensure};
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
#[derive(Debug)]
struct PdhError {
    code: u32,
    operation: &'static str,
}
impl std::fmt::Display for PdhError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: PDH 0x{:08x}", self.operation, self.code)
    }
}
impl std::error::Error for PdhError {}
fn check(code: u32, operation: &'static str) -> Result<()> {
    if code != 0 {
        return Err(PdhError { code, operation }.into());
    }
    Ok(())
}

fn transient(code: u32) -> bool {
    matches!(
        code,
        PDH_CSTATUS_NO_COUNTER
            | PDH_CSTATUS_NO_INSTANCE
            | PDH_CSTATUS_INVALID_DATA
            | PDH_INVALID_DATA
            | PDH_CSTATUS_ITEM_NOT_VALIDATED
            | PDH_NO_DATA
            | PDH_MORE_DATA
    )
}
fn transient_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PdhError>()
        .is_some_and(|error| transient(error.code))
}

type Values = Vec<(String, f64)>;
/// Re-registering the wildcard discards a stale instance expansion. Its first
/// collection primes percentages; the following 1 Hz poll supplies valid data.
fn read_or_refresh<T>(
    source: &mut T,
    read: fn(&mut T) -> Result<Values>,
    refresh: fn(&mut T) -> Result<()>,
) -> Result<Values> {
    let result = read(source);
    if result.as_ref().is_err_and(transient_error)
        && let Err(error) = refresh(source)
    {
        return Err(result
            .expect_err("failed read")
            .context(format!("refresh GPU wildcard: {error:#}")));
    }
    result
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
        if let Err(error) = sampler.refresh()
            && !transient_error(&error)
        {
            return Err(error);
        }
        Ok(sampler)
    }
    fn refresh(&mut self) -> Result<()> {
        if !self.counter.is_null() {
            // SAFETY: this query owns the counter; no read is in flight.
            let code = unsafe { PdhRemoveCounter(self.counter) };
            if code != 0 && !transient(code) {
                check(code, "remove stale GPU counter")?;
            }
            self.counter = ptr::null_mut();
        }
        let path: Vec<_> = r"\GPU Engine(*)\Utilization Percentage"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        check(
            // SAFETY: query is live, path terminated, output counter writable.
            unsafe { PdhAddEnglishCounterW(self.query, path.as_ptr(), 0, &mut self.counter) },
            "add per-process GPU counter",
        )?;
        // SAFETY: prime the live query; percentages require two collections.
        let code = unsafe { PdhCollectQueryData(self.query) };
        if !transient(code) {
            check(code, "prime GPU counters")?;
        }
        Ok(())
    }
    pub fn sample(&mut self) -> Result<Snapshot> {
        let engines = read_or_refresh(self, Self::read, Self::refresh)?;
        let processes = processes()?;
        Ok(Snapshot {
            unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
            processes,
            engines,
        })
    }
    fn read(&mut self) -> Result<Vec<(String, f64)>> {
        if self.counter.is_null() {
            self.refresh()?;
            return Err(PdhError {
                code: PDH_NO_DATA,
                operation: "prime refreshed GPU counter",
            }
            .into());
        }
        // SAFETY: query belongs to this sampler and remains open.
        let code = unsafe { PdhCollectQueryData(self.query) };
        check(code, "collect GPU counters")?;
        self.values()
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
            if code != PDH_MORE_DATA {
                check(code, "size GPU counters")?;
            }
            if bytes == 0 {
                return Err(PdhError {
                    code: PDH_NO_DATA,
                    operation: "empty GPU counters",
                }
                .into());
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
                // A retired instance is not loss of the remaining wildcard data.
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
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                if value.is_finite() && value >= 0. {
                    values.push((String::from_utf16_lossy(&name[..end]), value));
                }
            }
            if values.is_empty() {
                return Err(PdhError {
                    code: PDH_CSTATUS_INVALID_DATA,
                    operation: "no valid GPU counter instance",
                }
                .into());
            }
            return Ok(values);
        }
        Err(PdhError {
            code: PDH_MORE_DATA,
            operation: "GPU instances changed repeatedly while reading",
        }
        .into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disappearing_array_between_size_probe_and_read_refreshes_then_recovers() {
        struct Fake {
            status: u32,
            reads: u32,
            refreshes: u32,
        }
        fn read(fake: &mut Fake) -> Result<Values> {
            fake.reads += 1;
            if fake.reads == 1 {
                Err(PdhError {
                    code: fake.status,
                    operation: "read GPU counters",
                }
                .into())
            } else {
                Ok(vec![("pid_99_eng_0".into(), 0.)])
            }
        }
        fn refresh(fake: &mut Fake) -> Result<()> {
            fake.refreshes += 1;
            Ok(())
        }
        for status in [
            PDH_CSTATUS_NO_COUNTER,
            PDH_CSTATUS_NO_INSTANCE,
            PDH_CSTATUS_INVALID_DATA,
            PDH_INVALID_DATA,
        ] {
            let mut fake = Fake {
                status,
                reads: 0,
                refreshes: 0,
            };
            assert!(read_or_refresh(&mut fake, read, refresh).is_err());
            assert_eq!(fake.refreshes, 1);
            assert_eq!(read_or_refresh(&mut fake, read, refresh).unwrap().len(), 1);
            assert_eq!(fake.refreshes, 1);
        }
    }
}
