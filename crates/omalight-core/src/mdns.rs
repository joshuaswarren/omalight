//! DNS-SD advertisement for engines on the local network.
//!
//! The service type is `_omalight._tcp`. A MagicDNS name or any other hostname is not involved
//! here; those are ordinary addresses the client already knows how to dial.

use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_omalight._tcp.local.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub host: String,
    pub port: u16,
}

pub struct Advertiser {
    daemon: mdns_sd::ServiceDaemon,
}

pub fn advertise(instance: &str, port: u16) -> Result<Advertiser, String> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let instance = dns_name(instance);
    let host_label = dns_name(&crate::config::hostname());
    let host_name = format!("{host_label}.local.");
    let properties = [("role", "engine"), ("name", host_label.as_str())];
    let info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_name,
        "",
        port,
        &properties[..],
    )
    .map_err(|e| e.to_string())?
    .enable_addr_auto();
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(Advertiser { daemon })
}

impl Advertiser {
    pub fn browse(&self, timeout: Duration) -> Result<Vec<Found>, String> {
        browse_with(&self.daemon, timeout)
    }
}

pub fn browse(timeout: Duration) -> Result<Vec<Found>, String> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    browse_with(&daemon, timeout)
}

fn browse_with(daemon: &mdns_sd::ServiceDaemon, timeout: Duration) -> Result<Vec<Found>, String> {
    let receiver = daemon.browse(SERVICE_TYPE).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = match receiver.recv_timeout(left) {
            Ok(event) => event,
            Err(_) => break,
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
            let port = info.get_port();
            let name = info
                .get_properties()
                .get("name")
                .map(|v| v.val_str().to_string())
                .unwrap_or_else(|| info.get_fullname().to_string());
            let host = host_of(&info);
            if found
                .iter()
                .any(|item: &Found| item.host == host && item.port == port)
            {
                continue;
            }
            found.push(Found { name, host, port });
        }
    }
    Ok(found)
}

fn host_of(info: &mdns_sd::ServiceInfo) -> String {
    let port = info.get_port();
    let hostname = info.get_hostname().trim_end_matches('.').trim();
    if !hostname.is_empty() {
        return format!("{hostname}:{port}");
    }
    if let Some(ip) = info.get_addresses().iter().copied().next() {
        return format_addr(ip, port);
    }
    format!("127.0.0.1:{port}")
}

fn format_addr(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("{ip}:{port}"),
        IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    }
}

fn dns_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        } else if (c == ' ' || c == '_' || c == '.') && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "omalight".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_is_omalight() {
        assert_eq!(SERVICE_TYPE, "_omalight._tcp.local.");
        assert_eq!(dns_name("Mac Studio"), "Mac-Studio");
    }
}
