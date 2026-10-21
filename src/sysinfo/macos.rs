//! macOS: sysctl, Mach, CoreGraphics and a couple of cached tool calls.

use super::{read_trim, run, tidy_cpu};
use std::ffi::CString;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- sysctl

fn sysctl_raw(name: &str) -> Option<Vec<u8>> {
    let c = CString::new(name).ok()?;
    unsafe {
        let mut len: libc::size_t = 0;
        if libc::sysctlbyname(c.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 || len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(c.as_ptr(), buf.as_mut_ptr() as *mut _, &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        buf.truncate(len);
        Some(buf)
    }
}

fn sysctl_string(name: &str) -> Option<String> {
    let mut b = sysctl_raw(name)?;
    while b.last() == Some(&0) {
        b.pop();
    }
    let s = String::from_utf8_lossy(&b).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let b = sysctl_raw(name)?;
    match b.len() {
        8 => Some(u64::from_ne_bytes(b[..8].try_into().ok()?)),
        4 => Some(u32::from_ne_bytes(b[..4].try_into().ok()?) as u64),
        _ => None,
    }
}

// ---------------------------------------------------------------- identity

pub fn os_name() -> String {
    let name = run("sw_vers", &["-productName"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let ver = run("sw_vers", &["-productVersion"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match (name, ver) {
        (Some(n), Some(v)) => format!("{} {}", n, v),
        (Some(n), None) => n,
        _ => "macOS".into(),
    }
}

/// "MacBookPro18,3" style model identifier.
pub fn host_model() -> String {
    sysctl_string("hw.model").unwrap_or_default()
}

pub fn board() -> String {
    "Apple".into()
}

pub fn wm_de() -> (String, String) {
    ("Quartz Compositor".into(), "Aqua".into())
}

pub fn cpu_model() -> String {
    match sysctl_string("machdep.cpu.brand_string") {
        Some(s) => tidy_cpu(&s),
        None => "Unknown".into(),
    }
}

// ---------------------------------------------------------------- gpu, displays

fn cache_file(name: &str) -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.cache", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(base).join("termdash").join(name)
}

/// system_profiler takes the best part of a second, and the GPU never
/// changes, so ask once and keep the answer on disk.
pub fn gpu() -> String {
    let path = cache_file("gpu");
    if let Some(v) = read_trim(&path) {
        return v;
    }
    let mut found: Vec<String> = Vec::new();
    if let Some(out) = run("system_profiler", &["SPDisplaysDataType"]) {
        for line in out.lines() {
            if let Some(v) = line.trim().strip_prefix("Chipset Model:") {
                let v = v.trim().to_string();
                if !v.is_empty() && !found.contains(&v) {
                    found.push(v);
                }
            }
        }
    }
    if found.is_empty() {
        return "Unknown".into();
    }
    let v = found.join(", ");
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&path, &v);
    v
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
}

/// Size of each active display in points, as the desktop lays them out.
pub fn resolution() -> String {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    let ok = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } == 0;
    if !ok || n == 0 {
        return "Unknown".into();
    }
    ids[..n as usize]
        .iter()
        .map(|&d| unsafe { format!("{}x{}", CGDisplayPixelsWide(d), CGDisplayPixelsHigh(d)) })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------- terminal

pub fn terminal() -> String {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return "kitty".into();
    }
    if let Ok(p) = std::env::var("TERM_PROGRAM") {
        return match p.as_str() {
            "Apple_Terminal" => "Terminal".into(),
            "iTerm.app" => "iTerm2".into(),
            "" => std::env::var("TERM").unwrap_or_else(|_| "Unknown".into()),
            _ => p,
        };
    }
    std::env::var("TERM").unwrap_or_else(|_| "Unknown".into())
}

// ---------------------------------------------------------------- packages

pub fn count_packages() -> String {
    let mut parts: Vec<String> = Vec::new();
    // `brew list` is slow; counting the Cellar and Caskroom directories is not.
    for prefix in ["/opt/homebrew", "/usr/local"] {
        let count = |sub: &str| -> usize {
            fs::read_dir(format!("{}/{}", prefix, sub))
                .map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
                .unwrap_or(0)
        };
        let formulae = count("Cellar");
        let casks = count("Caskroom");
        if formulae > 0 {
            parts.push(format!("{} (brew)", formulae));
        }
        if casks > 0 {
            parts.push(format!("{} (brew-cask)", casks));
        }
        if !parts.is_empty() {
            break;
        }
    }
    if parts.is_empty() {
        "Unknown".into()
    } else {
        parts.join(", ")
    }
}

// ---------------------------------------------------------------- dynamic

pub fn uptime_secs() -> u64 {
    unsafe {
        let mut tv: libc::timeval = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::timeval>();
        let name = CString::new("kern.boottime").unwrap();
        if libc::sysctlbyname(name.as_ptr(), &mut tv as *mut _ as *mut _, &mut len, std::ptr::null_mut(), 0) != 0 {
            return 0;
        }
        let mut now: libc::timeval = std::mem::zeroed();
        libc::gettimeofday(&mut now, std::ptr::null_mut());
        (now.tv_sec - tv.tv_sec).max(0) as u64
    }
}

pub fn loadavg() -> String {
    let mut l = [0f64; 3];
    if unsafe { libc::getloadavg(l.as_mut_ptr(), 3) } != 3 {
        return "Unknown".into();
    }
    format!("{:.2} {:.2} {:.2}", l[0], l[1], l[2])
}

/// "/" is a read-only system snapshot on modern macOS; the user's data and
/// the free space live on the Data volume.
pub fn disk_path() -> &'static str {
    if std::path::Path::new("/System/Volumes/Data").exists() {
        "/System/Volumes/Data"
    } else {
        "/"
    }
}

/// (total, available, swap total, swap free) in bytes.
pub fn meminfo() -> (u64, u64, u64, u64) {
    let total = sysctl_u64("hw.memsize").unwrap_or(0);

    let mut avail = 0u64;
    unsafe {
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::vm_statistics64>() / std::mem::size_of::<libc::integer_t>()) as u32;
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let rc = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut _ as *mut libc::integer_t,
            &mut count,
        );
        if rc == 0 {
            let page = sysctl_u64("hw.pagesize").unwrap_or(16384);
            avail = (stats.free_count as u64 + stats.inactive_count as u64) * page;
        }
    }

    // struct xsw_usage { u64 total; u64 avail; u64 used; u32 pagesize; bool encrypted; }
    let (stotal, sfree) = match sysctl_raw("vm.swapusage") {
        Some(b) if b.len() >= 16 => (
            u64::from_ne_bytes(b[0..8].try_into().unwrap()),
            u64::from_ne_bytes(b[8..16].try_into().unwrap()),
        ),
        _ => (0, 0),
    };
    (total, avail, stotal, sfree)
}

static BATTERY: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// pmset is a process spawn, so reuse its answer for half a minute.
pub fn battery() -> String {
    let mut slot = BATTERY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = slot.as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return v.clone();
        }
    }
    let v = read_battery();
    *slot = Some((Instant::now(), v.clone()));
    v
}

fn read_battery() -> String {
    // "-InternalBattery-0 (id=1234)\t85%; discharging; 4:12 remaining present: true"
    if let Some(out) = run("pmset", &["-g", "batt"]) {
        for line in out.lines() {
            if !line.contains("InternalBattery") {
                continue;
            }
            let rest = line.split_once('\t').map(|x| x.1).unwrap_or(line);
            let mut parts = rest.split(';').map(|p| p.trim());
            let pct = parts.next().unwrap_or("").split('%').next().unwrap_or("").trim();
            let status = parts.next().unwrap_or("unknown");
            if !pct.is_empty() {
                let mut status = status.to_string();
                if let Some(c) = status.get(..1) {
                    status = c.to_uppercase() + &status[1..];
                }
                return format!("{}% [{}]", pct, status);
            }
        }
    }
    "No battery".into()
}
