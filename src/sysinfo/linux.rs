//! Linux: everything comes from /proc, /sys and os-release.

use super::{first_line, read_trim, run, tidy_cpu};
use std::fs;
use std::path::PathBuf;

fn dmi(name: &str) -> Option<String> {
    read_trim(format!("/sys/class/dmi/id/{}", name)).filter(|v| {
        !matches!(
            v.as_str(),
            "None" | "Not Specified" | "System Version" | "Default string"
                | "To Be Filled By O.E.M." | "To be filled by O.E.M." | "Unknown" | "1.0"
        )
    })
}

pub fn cpu_model() -> String {
    if let Ok(t) = fs::read_to_string("/proc/cpuinfo") {
        for line in t.lines() {
            // x86 says "model name", ARM says "Model name" or "Hardware"
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("model name") || lower.starts_with("hardware") {
                if let Some((_, v)) = line.split_once(':') {
                    return tidy_cpu(v.trim());
                }
            }
        }
    }
    read_trim("/sys/firmware/devicetree/base/model").unwrap_or_else(|| "Unknown".into())
}

/// Look a PCI vendor/device pair up in the hwdata database.
fn pci_name(vendor: u16, device: u16) -> Option<(String, String)> {
    let db = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())?;
    let vkey = format!("{:04x}", vendor);
    let dkey = format!("{:04x}", device);
    let mut vname = None;
    let mut in_vendor = false;
    for line in db.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                break; // next vendor started, device was not listed
            }
            if let Some(rest) = line.strip_prefix(&vkey) {
                if rest.starts_with("  ") {
                    vname = Some(rest.trim().to_string());
                    in_vendor = true;
                }
            }
        } else if in_vendor && !line.starts_with("\t\t") {
            let l = line.trim_start();
            if let Some(rest) = l.strip_prefix(&dkey) {
                if rest.starts_with("  ") {
                    return Some((vname.unwrap_or_default(), rest.trim().to_string()));
                }
            }
        }
    }
    vname.map(|v| (v, String::new()))
}

pub fn gpu() -> String {
    let mut found: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") {
        let mut devs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        devs.sort();
        for d in devs {
            let class = read_trim(d.join("class")).unwrap_or_default();
            // 0x0300xx = VGA, 0x0302xx = 3D controller, 0x0380xx = display
            if !(class.starts_with("0x0300") || class.starts_with("0x0302") || class.starts_with("0x0380")) {
                continue;
            }
            let vid = read_trim(d.join("vendor")).and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok());
            let did = read_trim(d.join("device")).and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok());
            if let (Some(v), Some(dd)) = (vid, did) {
                if let Some((vn, dn)) = pci_name(v, dd) {
                    // pci.ids writes the marketing name in [brackets]
                    let name = match (dn.find('['), dn.rfind(']')) {
                        (Some(a), Some(b)) if b > a + 1 => dn[a + 1..b].to_string(),
                        _ => dn.clone(),
                    };
                    let vn = short_vendor(&vn);
                    found.push(if name.is_empty() { vn } else { format!("{} {}", vn, name) });
                } else {
                    found.push(format!("{:04x}:{:04x}", v, dd));
                }
            }
        }
    }
    if found.is_empty() {
        if let Some(out) = run("lspci", &["-mm"]) {
            for line in out.lines() {
                if line.contains("VGA") || line.contains("3D controller") || line.contains("Display controller") {
                    let parts: Vec<&str> = line.split('"').collect();
                    if parts.len() > 6 {
                        found.push(format!("{} {}", short_vendor(parts[3]), parts[5]));
                    }
                }
            }
        }
    }
    if found.is_empty() {
        "Unknown".into()
    } else {
        found.join(", ")
    }
}

pub fn resolution() -> String {
    let mut out: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            if read_trim(d.join("status")).as_deref() != Some("connected") {
                continue;
            }
            if let Some(modes) = fs::read_to_string(d.join("modes")).ok() {
                let m = first_line(&modes).trim();
                if !m.is_empty() {
                    out.push(m.to_string());
                }
            }
        }
    }
    if out.is_empty() {
        "Unknown".into()
    } else {
        out.join(", ")
    }
}

/// Walk up the process tree looking for something that looks like a terminal.
pub fn terminal() -> String {
    const KNOWN: &[&str] = &[
        "kitty", "ghostty", "ptyxis", "gnome-terminal-server", "gnome-terminal", "konsole",
        "alacritty", "foot", "wezterm-gui", "wezterm", "tilix", "terminator", "xterm",
        "xfce4-terminal", "urxvt", "st", "contour", "rio", "warp", "hyper", "tmux", "screen",
    ];
    let mut pid = std::process::id();
    for _ in 0..12 {
        let stat = match fs::read_to_string(format!("/proc/{}/stat", pid)) {
            Ok(s) => s,
            Err(_) => break,
        };
        // comm is in parentheses and may itself contain spaces
        let (_open, close) = match (stat.find('('), stat.rfind(')')) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => break,
        };
        let ppid: u32 = match stat[close + 1..].split_whitespace().nth(1).and_then(|v| v.parse().ok()) {
            Some(p) => p,
            None => break,
        };
        if ppid <= 1 {
            break;
        }
        let name = fs::read_to_string(format!("/proc/{}/comm", ppid))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if let Some(k) = KNOWN.iter().find(|k| lower == **k || lower.starts_with(*k)) {
            return match *k {
                "gnome-terminal-server" => "gnome-terminal".to_string(),
                "wezterm-gui" => "wezterm".to_string(),
                other => other.to_string(),
            };
        }
        pid = ppid;
    }
    for var in ["TERM_PROGRAM", "TERMINAL_EMULATOR"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    std::env::var("TERM").unwrap_or_else(|_| "Unknown".into())
}

pub fn count_packages() -> String {
    let mut parts: Vec<String> = Vec::new();
    let managers: &[(&str, &[&str], &str)] = &[
        ("rpm", &["-qa", "--qf", "x\n"], "rpm"),
        ("dpkg-query", &["-f", "x\n", "-W"], "dpkg"),
        ("pacman", &["-Qq"], "pacman"),
    ];
    for (bin, args, label) in managers {
        if let Some(out) = run(bin, args) {
            let n = out.lines().filter(|l| !l.trim().is_empty()).count();
            if n > 0 {
                parts.push(format!("{} ({})", n, label));
                break;
            }
        }
    }
    if let Some(out) = run("flatpak", &["list", "--columns=application"]) {
        let n = out.lines().filter(|l| !l.trim().is_empty()).count();
        if n > 0 {
            parts.push(format!("{} (flatpak)", n));
        }
    }
    if parts.is_empty() {
        "Unknown".into()
    } else {
        parts.join(", ")
    }
}

pub fn meminfo() -> (u64, u64, u64, u64) {
    let (mut total, mut avail, mut stotal, mut sfree) = (0u64, 0u64, 0u64, 0u64);
    if let Ok(t) = fs::read_to_string("/proc/meminfo") {
        for line in t.lines() {
            let mut it = line.split_whitespace();
            let key = it.next().unwrap_or("");
            let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            match key {
                "MemTotal:" => total = val,
                "MemAvailable:" => avail = val,
                "SwapTotal:" => stotal = val,
                "SwapFree:" => sfree = val,
                _ => {}
            }
        }
    }
    (total * 1024, avail * 1024, stotal * 1024, sfree * 1024)
}

pub fn battery() -> String {
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            if read_trim(d.join("type")).as_deref() != Some("Battery") {
                continue;
            }
            if let Some(cap) = read_trim(d.join("capacity")) {
                let status = read_trim(d.join("status")).unwrap_or_else(|| "Unknown".into());
                return format!("{}% [{}]", cap, status);
            }
        }
    }
    "No battery".into()
}


fn os_release(key: &str) -> Option<String> {
    let text = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub fn os_name() -> String {
    os_release("PRETTY_NAME").unwrap_or_else(|| "Linux".into())
}

pub fn host_model() -> String {
    let mut host = dmi("product_name").unwrap_or_default();
    if let Some(v) = dmi("product_version") {
        if !host.is_empty() {
            host.push(' ');
        }
        host.push_str(&v);
    }
    host
}

pub fn board() -> String {
    match (dmi("board_vendor"), dmi("board_name")) {
        (Some(v), Some(n)) => format!("{} {}", v, n),
        (Some(v), None) => v,
        (None, Some(n)) => n,
        (None, None) => "Unknown".into(),
    }
}

/// (window manager, desktop environment)
pub fn wm_de() -> (String, String) {
    let de = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_else(|_| "Unknown".into());
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let wm = if session.is_empty() { de.clone() } else { format!("{} ({})", de, session) };
    (wm, de)
}

pub fn uptime_secs() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(0.0) as u64
}

pub fn loadavg() -> String {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "Unknown".into())
}

pub fn disk_path() -> &'static str {
    "/"
}

fn short_vendor(v: &str) -> String {
    if v.contains("Advanced Micro Devices") || v.starts_with("AMD") {
        return "AMD".into();
    }
    if v.contains("NVIDIA") {
        return "NVIDIA".into();
    }
    if v.contains("Intel") {
        return "Intel".into();
    }
    v.replace(" Corporation", "").replace(", Inc.", "").replace(" Inc.", "")
}
