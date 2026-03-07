use crate::state::Instance;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

const SUBNET_PREFIX: &str = "192.168.105";
const SUBNET_CIDR: u8 = 24;
const GATEWAY_OCTET: u8 = 1;
const DHCP_END_OCTET: u8 = 254;
const STATIC_START_OCTET: u8 = 2;
const STATIC_END_OCTET: u8 = 127;

#[derive(Clone, Debug)]
pub struct VmnetConfig {
    pub client_path: PathBuf,
    pub interface_id: String,
    pub mac_address: String,
    pub guest_ip: String,
    pub gateway_ip: String,
    pub prefix_len: u8,
    pub dhcp_start: String,
    pub dhcp_end: String,
    pub dns_servers: Vec<String>,
}

impl VmnetConfig {
    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("guest_ip: {}", self.guest_ip),
            format!("guest_gateway: {}", self.gateway_ip),
            format!("guest_mac: {}", self.mac_address),
            format!("vmnet_client: {}", self.client_path.display()),
        ]
    }
}

pub fn resolve_for_instance(instance: &Instance) -> Result<VmnetConfig, String> {
    let client_path = find_vmnet_client().ok_or_else(|| {
        "vmnet-client is not installed; install vmnet-helper first: curl -fsSL https://raw.githubusercontent.com/nirs/vmnet-helper/main/install.sh | sudo bash".to_string()
    })?;

    let mut hasher = DefaultHasher::new();
    instance.id.hash(&mut hasher);
    let digest = hasher.finish();

    let static_span = STATIC_END_OCTET - STATIC_START_OCTET + 1;
    let guest_octet = STATIC_START_OCTET + ((digest as u8) % static_span);
    let guest_ip = format!("{SUBNET_PREFIX}.{guest_octet}");
    let gateway_ip = format!("{SUBNET_PREFIX}.{GATEWAY_OCTET}");
    let dhcp_start = gateway_ip.clone();
    let dhcp_end = format!("{SUBNET_PREFIX}.{DHCP_END_OCTET}");

    Ok(VmnetConfig {
        client_path,
        interface_id: format_uuid(digest),
        mac_address: format_mac(digest),
        guest_ip,
        gateway_ip,
        prefix_len: SUBNET_CIDR,
        dhcp_start,
        dhcp_end,
        dns_servers: vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()],
    })
}

pub fn find_vmnet_client() -> Option<PathBuf> {
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join("vmnet-client");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let fallback = PathBuf::from("/opt/vmnet-helper/bin/vmnet-client");
    if fallback.is_file() {
        Some(fallback)
    } else {
        None
    }
}

fn format_uuid(digest: u64) -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&digest.to_be_bytes());
    bytes[8..].copy_from_slice(&(!digest).to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn format_mac(digest: u64) -> String {
    let bytes = digest.to_be_bytes();
    format!(
        "52:54:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

#[cfg(test)]
mod tests {
    use super::{format_mac, format_uuid};

    #[test]
    fn uuid_is_stable() {
        assert_eq!(
            format_uuid(0x0123_4567_89ab_cdef),
            "01234567-89ab-4def-bedc-ba9876543210"
        );
    }

    #[test]
    fn mac_is_locally_administered() {
        let mac = format_mac(0x0123_4567_89ab_cdef);
        assert_eq!(mac, "52:54:89:ab:cd:ef");
    }
}
