//! mDNS announcement for the Android app's server discovery (`bonjour-service` → mdns-sd):
//! `_peekaround._tcp`, IPv4 only, its own `peekaround-<label>.local` host name, TXT `v=<version>`.

use crate::config::{interfaces, mdns_service};

/// A published service (unpublished on drop / stop).
pub struct Mdns {
    daemon: mdns_sd::ServiceDaemon,
    fullname: String,
}

impl Mdns {
    /// publish `hostname` on `port`; None when mDNS is unavailable
    pub fn publish(hostname: &str, port: u16, version: &str) -> Option<Mdns> {
        let s = mdns_service(hostname, port, version);
        let daemon = mdns_sd::ServiceDaemon::new().ok()?;
        let _ = daemon.disable_interface(mdns_sd::IfKind::IPv6);
        let ips: Vec<std::net::IpAddr> = interfaces()
            .into_iter()
            .filter(|i| !i.internal)
            .map(|i| std::net::IpAddr::V4(i.addr))
            .collect();
        let ty = format!("_{}._tcp.local.", s.kind);
        let host = format!("{}.", s.host);
        let props = [("v", s.txt_v.as_str())];
        let info =
            mdns_sd::ServiceInfo::new(&ty, &s.name, &host, &ips[..], s.port, &props[..]).ok()?;
        let fullname = info.get_fullname().to_string();
        daemon.register(info).ok()?;
        Some(Mdns { daemon, fullname })
    }

    /// unpublish
    pub fn stop(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
        }
        let _ = self.daemon.shutdown();
    }
}
