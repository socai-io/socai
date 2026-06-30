//! Host machine info, collected once and shared so every consumer reports the
//! same values. Both telemetry (uploaded device fields) and OCR diagnostics
//! (the local `ocr_perf.json`) read from here, so reported and local machine
//! info never drift apart.

use std::sync::OnceLock;

/// Cached snapshot of the host's identifying parameters.
#[derive(Debug, Clone)]
pub struct MachineInfo {
    /// `std::env::consts::OS`, e.g. "macos", "linux", "windows".
    pub os: &'static str,
    /// `std::env::consts::ARCH`, e.g. "aarch64", "x86_64".
    pub arch: &'static str,
    /// Concrete CPU/chip model, e.g. "Apple M4". Empty when undetectable.
    pub cpu_model: String,
    /// Logical CPU count.
    pub cpu_count: Option<usize>,
    /// Total physical RAM in MiB.
    pub memory_total_mb: Option<u64>,
    /// OS product version, e.g. macOS "15.7.3".
    pub os_version: String,
    /// OS kernel version (`uname -r` on Unix).
    pub os_kernel_version: String,
}

/// Process-global machine info, collected on first use.
pub fn machine_info() -> &'static MachineInfo {
    static INFO: OnceLock<MachineInfo> = OnceLock::new();
    INFO.get_or_init(|| MachineInfo {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        cpu_model: cpu_model(),
        cpu_count: std::thread::available_parallelism()
            .ok()
            .map(|count| count.get()),
        memory_total_mb: memory_total_mb(),
        os_version: os_version(),
        os_kernel_version: os_kernel_version(),
    })
}

/// Best-effort concrete CPU/chip model, e.g. "Apple M4" on macOS,
/// "Intel(R) Core(TM) i7-…" on Linux, the PROCESSOR_IDENTIFIER on Windows.
/// Empty string when it can't be determined.
fn cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        let s = command_output("sysctl", &["-n", "machdep.cpu.brand_string"]);
        if !s.is_empty() {
            return s;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(txt) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in txt.lines() {
                if let Some(rest) = line.strip_prefix("model name") {
                    if let Some(idx) = rest.find(':') {
                        let s = rest[idx + 1..].trim().to_string();
                        if !s.is_empty() {
                            return s;
                        }
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(v) = std::env::var("PROCESSOR_IDENTIFIER") {
            let s = v.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        return command_output("sw_vers", &["-productVersion"]);
    }
    #[cfg(target_os = "linux")]
    {
        return linux_pretty_name().unwrap_or_default();
    }
    #[cfg(target_os = "windows")]
    {
        return command_output("cmd", &["/C", "ver"]);
    }
    #[allow(unreachable_code)]
    String::new()
}

fn os_kernel_version() -> String {
    #[cfg(unix)]
    {
        return command_output("uname", &["-r"]);
    }
    #[cfg(target_os = "windows")]
    {
        return command_output("cmd", &["/C", "ver"]);
    }
    #[allow(unreachable_code)]
    String::new()
}

#[cfg(target_os = "linux")]
fn linux_pretty_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        let Some(value) = line.strip_prefix("PRETTY_NAME=") else {
            continue;
        };
        return Some(value.trim_matches('"').to_string());
    }
    None
}

#[cfg(target_os = "macos")]
fn memory_total_mb() -> Option<u64> {
    use std::ffi::CString;
    let name = CString::new("hw.memsize").ok()?;
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Some(value / 1024 / 1024)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn memory_total_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
        return Some(kb / 1024);
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn memory_total_mb() -> Option<u64> {
    None
}

/// Run a command and return its trimmed stdout, or empty string on failure.
/// Shared with telemetry (parent-process / terminal detection).
pub(crate) fn command_output(program: &str, args: &[&str]) -> String {
    let Ok(output) = std::process::Command::new(program).args(args).output() else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
