//! Server options (mirrors `config.ts`): what `createServer` takes, the resolved data layout and
//! the LAN addresses shown to phones.

use pa_core::RunMode;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

/// Desktop integration points.
pub trait DesktopHooks: Send + Sync {
    /// move a file (or directory) to the OS trash; None = not supported (→ `.peekaround-trash`)
    fn trash(&self, _abs: &std::path::Path) -> Option<std::io::Result<()>> {
        None
    }
    /// true when [`DesktopHooks::trash`] is implemented
    fn has_trash(&self) -> bool {
        false
    }
}

/// Browser security options (Host / Origin allow-lists).
#[derive(Debug, Clone, Default)]
pub struct SecurityOptions {
    /// extra Host names (reverse proxy domains); '*' disables the Host check (env PEEK_ALLOWED_HOSTS)
    pub allowed_hosts: Vec<String>,
    /// extra browser origins allowed to call the API (env PEEK_ALLOWED_ORIGINS)
    pub allowed_origins: Vec<String>,
}

fn split_list(v: Option<String>) -> Vec<String> {
    v.unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

impl SecurityOptions {
    /// from PEEK_ALLOWED_HOSTS / PEEK_ALLOWED_ORIGINS
    pub fn from_env() -> Self {
        SecurityOptions {
            allowed_hosts: split_list(std::env::var("PEEK_ALLOWED_HOSTS").ok()),
            allowed_origins: split_list(std::env::var("PEEK_ALLOWED_ORIGINS").ok()),
        }
    }
}

/// A library root given on the command line / in the options.
#[derive(Debug, Clone)]
pub struct RootSpec {
    pub path: String,
    pub name: Option<String>,
    /// one library per sub folder
    pub split: bool,
}

/// `ServerOptions` of `createServer`.
#[derive(Clone)]
pub struct ServerOptions {
    /// Host / Origin allow-lists (default: from the environment)
    pub security: Option<SecurityOptions>,
    /// 'server' (default) or 'desktop'
    pub mode: RunMode,
    /// bind address (default 0.0.0.0)
    pub host: String,
    /// port (default 8080; 0 = any free port)
    pub port: u16,
    /// data / cache directory
    pub data_dir: PathBuf,
    /// folders that become libraries on first start
    pub roots: Vec<RootSpec>,
    /// built web UI to serve
    pub web_dir: Option<PathBuf>,
    /// ffmpeg binary (default $PEEK_FFMPEG or "ffmpeg")
    pub ffmpeg_path: Option<String>,
    /// ffprobe binary (default $PEEK_FFPROBE or "ffprobe")
    pub ffprobe_path: Option<String>,
    /// no uploads / edits / deletes
    pub readonly: bool,
    /// poll for file changes (network shares)
    pub watch_poll: bool,
    /// announce on the local network
    pub mdns: bool,
    /// directories the "add folder" host browser can show
    pub host_browse_roots: Vec<String>,
    /// upload size limit
    pub max_upload_bytes: u64,
    /// log requests
    pub logger: bool,
    /// desktop hooks (OS trash)
    pub hooks: Option<Arc<dyn DesktopHooks>>,
    /// version reported by /api/info
    pub version: String,
}

impl ServerOptions {
    /// defaults for a data dir
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        ServerOptions {
            security: None,
            mode: RunMode::Server,
            host: "0.0.0.0".into(),
            port: 8080,
            data_dir: data_dir.into(),
            roots: Vec::new(),
            web_dir: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            readonly: false,
            watch_poll: false,
            mdns: false,
            host_browse_roots: Vec::new(),
            max_upload_bytes: 20 * 1024 * 1024 * 1024,
            logger: false,
            hooks: None,
            version: "1.3.2".into(),
        }
    }
}

/// Resolved configuration (`ResolvedConfig`).
#[derive(Clone)]
pub struct ResolvedConfig {
    pub mode: RunMode,
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub roots: Vec<RootSpec>,
    pub web_dir: Option<PathBuf>,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub readonly: bool,
    pub watch_poll: bool,
    pub mdns: bool,
    pub host_browse_roots: Vec<PathBuf>,
    pub max_upload_bytes: u64,
    pub logger: bool,
    pub hooks: Option<Arc<dyn DesktopHooks>>,
    pub version: String,
    pub cache_dir: PathBuf,
    pub thumb_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub font_dir: PathBuf,
    pub db_path: PathBuf,
}

impl ResolvedConfig {
    /// `resolveConfig`
    pub fn resolve(o: &ServerOptions) -> Self {
        let data_dir = crate::paths::resolve(&o.data_dir);
        let cache_dir = data_dir.join("cache");
        ResolvedConfig {
            mode: o.mode,
            host: o.host.clone(),
            port: o.port,
            roots: o.roots.clone(),
            web_dir: o.web_dir.clone(),
            ffmpeg_path: o
                .ffmpeg_path
                .clone()
                .or_else(|| std::env::var("PEEK_FFMPEG").ok())
                .unwrap_or_else(|| "ffmpeg".into()),
            ffprobe_path: o
                .ffprobe_path
                .clone()
                .or_else(|| std::env::var("PEEK_FFPROBE").ok())
                .unwrap_or_else(|| "ffprobe".into()),
            readonly: o.readonly,
            watch_poll: o.watch_poll,
            mdns: o.mdns,
            host_browse_roots: o
                .host_browse_roots
                .iter()
                .map(crate::paths::resolve)
                .collect(),
            max_upload_bytes: o.max_upload_bytes,
            logger: o.logger,
            hooks: o.hooks.clone(),
            version: o.version.clone(),
            thumb_dir: cache_dir.join("thumbs"),
            temp_dir: data_dir.join("temp"),
            font_dir: data_dir.join("fonts"),
            db_path: data_dir.join("peekaround.db"),
            cache_dir,
            data_dir,
        }
    }

    /// desktop app mode
    pub fn desktop(&self) -> bool {
        self.mode == RunMode::Desktop
    }
}

/// Adapters a phone on the Wi-Fi is never on (Hyper-V / WSL, VirtualBox, VMware, Docker, VPNs).
fn is_virtual_adapter(name: &str) -> bool {
    let n = name.to_lowercase();
    if n.starts_with("br-") || n.starts_with("veth") {
        return true;
    }
    let wsl = n
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| w == "wsl");
    wsl || [
        "vethernet",
        "hyper-v",
        "virtualbox",
        "vbox",
        "vmware",
        "vmnet",
        "docker",
        "virbr",
        "tailscale",
        "zerotier",
    ]
    .iter()
    .any(|k| n.contains(k))
}

fn is_private_v4(a: Ipv4Addr) -> bool {
    let [x, y, ..] = a.octets();
    x == 10 || (x == 172 && (16..=31).contains(&y)) || (x == 192 && y == 168)
}

/// One network interface address (name, IPv4, internal).
#[derive(Debug, Clone)]
pub struct IfaceAddr {
    pub name: String,
    pub addr: Ipv4Addr,
    /// loopback
    pub internal: bool,
}

/// IPv4 addresses of this machine
pub fn interfaces() -> Vec<IfaceAddr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(ref v4) => Some(IfaceAddr {
                internal: i.is_loopback(),
                addr: v4.ip,
                name: i.name,
            }),
            _ => None,
        })
        .collect()
}

/// `lanUrls` over a given interface list (tests).
pub fn lan_urls_of(ifaces: &[IfaceAddr], port: u16) -> Vec<String> {
    let mut out: Vec<(String, u8, usize)> = Vec::new();
    for ni in ifaces {
        if ni.internal || ni.addr.octets()[0] == 169 && ni.addr.octets()[1] == 254 {
            continue;
        }
        let rank = (if is_virtual_adapter(&ni.name) { 2 } else { 0 })
            + (if is_private_v4(ni.addr) { 0 } else { 1 });
        let i = out.len();
        out.push((format!("http://{}:{port}", ni.addr), rank, i));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    out.into_iter().map(|o| o.0).collect()
}

/// http://<ip>:<port> of this machine for other devices on the network, most likely first
pub fn lan_urls(port: u16) -> Vec<String> {
    lan_urls_of(&interfaces(), port)
}

/// DNS-SD announcement (`mdnsService`)
#[derive(Debug, Clone, PartialEq)]
pub struct MdnsService {
    pub name: String,
    pub kind: String,
    pub port: u16,
    pub host: String,
    pub disable_ipv6: bool,
    pub txt_v: String,
}

/// `mdnsService(hostname, port, version)`: IPv4 only, its own `peekaround-<label>.local` host name
pub fn mdns_service(hostname: &str, port: u16, version: &str) -> MdnsService {
    let lower = hostname.to_lowercase();
    let mut label = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            label.push(c);
            dash = false;
        } else if !dash {
            label.push('-');
            dash = true;
        }
    }
    // /[^a-z0-9-]+/g → '-' (runs of '-' that were in the input stay)
    let label = label.trim_matches('-');
    let label: String = label.chars().take(40).collect();
    let label = if label.is_empty() {
        "pc".to_string()
    } else {
        label
    };
    let name: String = format!("PeekAround {hostname}").chars().take(60).collect();
    MdnsService {
        name,
        kind: "peekaround".into(),
        port,
        host: format!("peekaround-{label}.local"),
        disable_ipv6: true,
        txt_v: version.into(),
    }
}

/// `os.hostname()`
pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_names() {
        let s = mdns_service("DESKTOP-AB12CD", 8080, "1.3.0");
        assert_eq!(s.name, "PeekAround DESKTOP-AB12CD");
        assert_eq!(s.host, "peekaround-desktop-ab12cd.local");
        assert_eq!(
            mdns_service("Melikşah’s PC", 1, "x").host,
            "peekaround-melik-ah-s-pc.local"
        );
        assert_eq!(mdns_service("---", 1, "x").host, "peekaround-pc.local");
    }

    #[test]
    fn lan_order() {
        let v = |n: &str, a: &str| IfaceAddr {
            name: n.into(),
            addr: a.parse().unwrap(),
            internal: false,
        };
        let list = vec![
            v("vEthernet (WSL (Hyper-V firewall))", "172.29.16.1"),
            v("VirtualBox Host-Only Network", "192.168.56.1"),
            v("Ethernet 2", "169.254.12.3"),
            v("Wi-Fi", "192.168.1.23"),
            IfaceAddr {
                name: "lo".into(),
                addr: "127.0.0.1".parse().unwrap(),
                internal: true,
            },
        ];
        assert_eq!(
            lan_urls_of(&list, 8080),
            vec![
                "http://192.168.1.23:8080",
                "http://172.29.16.1:8080",
                "http://192.168.56.1:8080"
            ]
        );
    }
}
