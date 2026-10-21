//! System facts, read straight from /proc, /sys, sysctl and libc rather than
//! by spawning fastfetch, lspci, dpkg and friends. Everything here is cheap
//! except the package count, which is cached on disk and refreshed in the
//! background.
//!
//! The parts that differ per OS live in `linux.rs` and `macos.rs`; both
//! export the same set of functions and this file only stitches them together.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// Facts that do not change while the program runs.
#[derive(Debug, Clone, Default)]
pub struct Static {
    pub user: String,
    pub hostname: String,
    pub os: String,
    pub host: String,
    pub kernel: String,
    pub board: String,
    pub shell: String,
    pub resolution: String,
    pub terminal: String,
    pub cpu: String,
    pub gpu: String,
    pub wm: String,
    pub de: String,
    pub locale: String,
    pub packages: String,
}

/// Facts worth re-reading as the program runs.
#[derive(Debug, Clone, Default)]
pub struct Dynamic {
    pub uptime: String,
    pub memory: String,
    pub swap: String,
    pub disk: String,
    pub loadavg: String,
    pub battery: String,
    pub ip: String,
}

// ---------------------------------------------------------------- helpers

fn uname_field(which: usize) -> String {
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return String::new();
        }
        let p = match which {
            0 => u.sysname.as_ptr(),
            1 => u.nodename.as_ptr(),
            3 => u.machine.as_ptr(),
            _ => u.release.as_ptr(),
        };
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

pub fn hostname() -> String {
    let n = uname_field(1);
    if n.is_empty() {
        read_trim("/etc/hostname").unwrap_or_else(|| "localhost".into())
    } else {
        n
    }
}

fn bytes_human(b: u64) -> String {
    let gib = b as f64 / (1u64 << 30) as f64;
    if gib >= 1.0 {
        format!("{:.2} GiB", gib)
    } else {
        format!("{:.0} MiB", b as f64 / (1u64 << 20) as f64)
    }
}

// ---------------------------------------------------------------- pieces

fn tidy_cpu(s: &str) -> String {
    let mut out = s.replace("(R)", "").replace("(TM)", "").replace("(r)", "").replace("(tm)", "");
    // "AMD Ryzen 7 5800X 8-Core Processor" -> drop the core count and the word
    if let Some(i) = out.find("-Core Processor") {
        let head = &out[..i];
        let cut = head.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end().len();
        out = out[..cut].to_string();
    }
    for junk in [" @ ", " with ", " CPU"] {
        if let Some(i) = out.find(junk) {
            out = out[..i].to_string();
        }
    }
    if let Some(stripped) = out.strip_suffix(" Processor") {
        out = stripped.to_string();
    }
    let mut cleaned = String::with_capacity(out.len());
    let mut last_space = false;
    for ch in out.trim().chars() {
        if ch == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        cleaned.push(ch);
    }
    if cleaned.is_empty() {
        "Unknown".into()
    } else {
        cleaned
    }
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(cmd)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn shell() -> String {
    let path = std::env::var("SHELL").unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or("sh").to_string();
    if name.is_empty() {
        return "Unknown".into();
    }
    let ver = match name.as_str() {
        "zsh" => run(&path, &["-c", "echo $ZSH_VERSION"]).map(|s| s.trim().to_string()),
        "bash" => run(&path, &["-c", "echo $BASH_VERSION"]).map(|s| {
            s.trim().split('(').next().unwrap_or("").to_string()
        }),
        _ => run(&path, &["--version"]).map(|s| {
            first_line(&s)
                .split_whitespace()
                .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
                .unwrap_or("")
                .to_string()
        }),
    };
    match ver {
        Some(v) if !v.is_empty() => format!("{} {}", name, v),
        _ => name,
    }
}

// ---------------------------------------------------------------- packages

fn cache_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.cache", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(base).join("termdash/packages")
}

/// Return the cached package count, and refresh it in the background when stale.
/// `notify` is told the new value when a refresh finishes.
pub fn packages_cached(max_age: Duration, notify: Option<Sender<String>>) -> String {
    let path = cache_path();
    let cached = fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let fresh = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < max_age)
        .unwrap_or(false);

    if let (Some(v), true) = (cached.clone(), fresh) {
        return v;
    }
    match cached {
        // Stale but usable: show it now, refresh behind the scenes.
        Some(v) => {
            std::thread::spawn(move || {
                let fresh = os::count_packages();
                let _ = fs::create_dir_all(path.parent().unwrap());
                let _ = fs::write(&path, &fresh);
                if let Some(tx) = notify {
                    let _ = tx.send(fresh);
                }
            });
            v
        }
        // Nothing cached: we have to count now, or the panel has a blank row.
        None => {
            let v = os::count_packages();
            let _ = fs::create_dir_all(path.parent().unwrap());
            let _ = fs::write(&path, &v);
            v
        }
    }
}

// ---------------------------------------------------------------- collect

pub fn collect_static(title_os_override: &str, packages: String) -> Static {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".into());
    let host_name = hostname();

    let os_name = if !title_os_override.is_empty() {
        title_os_override.to_string()
    } else {
        let machine = uname_field(3);
        let name = os::os_name();
        if machine.is_empty() {
            name
        } else {
            format!("{} {}", name, machine)
        }
    };

    let mut host = os::host_model();
    if host.is_empty() {
        host = host_name.clone();
    }

    let (wm, de) = os::wm_de();

    Static {
        user,
        hostname: host_name,
        os: os_name,
        host,
        kernel: uname_field(2),
        board: os::board(),
        shell: shell(),
        resolution: os::resolution(),
        terminal: os::terminal(),
        cpu: os::cpu_model(),
        gpu: os::gpu(),
        wm,
        de,
        locale: std::env::var("LANG").unwrap_or_else(|_| "Unknown".into()),
        packages,
    }
}

fn disk_usage(path: &str) -> Option<(u64, u64)> {
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        let c = std::ffi::CString::new(path).ok()?;
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        let bs = s.f_frsize as u64;
        let total = s.f_blocks as u64 * bs;
        let used = total - s.f_bfree as u64 * bs;
        Some((used, total))
    }
}

fn local_ip() -> String {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return "127.0.0.1".into();
        }
        let mut best = String::new();
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy();
                let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = u32::from_be(sa.sin_addr.s_addr);
                let s = format!("{}.{}.{}.{}", ip >> 24, (ip >> 16) & 255, (ip >> 8) & 255, ip & 255);
                if name != "lo" && !s.starts_with("127.") && best.is_empty() {
                    best = s;
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        if best.is_empty() {
            "127.0.0.1".into()
        } else {
            best
        }
    }
}

pub fn collect_dynamic() -> Dynamic {
    let secs = os::uptime_secs();
    let uptime = {
        let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
        let s = secs % 60;
        if d > 0 {
            format!("{} days {} hours {} mins", d, h, m)
        } else if h > 0 {
            format!("{} hours {} mins {} secs", h, m, s)
        } else {
            format!("{} mins {} secs", m, s)
        }
    };

    let (mtotal, mavail, stotal, sfree) = os::meminfo();
    let mused = mtotal.saturating_sub(mavail);
    let memory = if mtotal > 0 {
        format!("{} / {} ({}%)", bytes_human(mused), bytes_human(mtotal), mused * 100 / mtotal)
    } else {
        "Unknown".into()
    };
    let swap = if stotal > 0 {
        let used = stotal.saturating_sub(sfree);
        format!("{} / {} ({}%)", bytes_human(used), bytes_human(stotal), used * 100 / stotal)
    } else {
        "None".into()
    };

    let disk = match disk_usage(os::disk_path()) {
        Some((u, t)) if t > 0 => {
            format!("{} / {} ({}%)", bytes_human(u), bytes_human(t), u * 100 / t)
        }
        _ => "Unknown".into(),
    };

    Dynamic {
        uptime,
        memory,
        swap,
        disk,
        loadavg: os::loadavg(),
        battery: os::battery(),
        ip: local_ip(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_tidy() {
        assert_eq!(tidy_cpu("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"), "Intel Core i7-8550U");
        assert_eq!(tidy_cpu("AMD Ryzen 7 5800X 8-Core Processor"), "AMD Ryzen 7 5800X");
    }

    #[test]
    fn basics_do_not_panic() {
        let d = collect_dynamic();
        assert!(!d.uptime.is_empty());
        assert!(!d.memory.is_empty());
        assert!(!d.ip.is_empty());
        let s = collect_static("", "0 (pkgs)".into());
        assert!(!s.kernel.is_empty());
        assert!(!s.cpu.is_empty());
    }
}
