use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const PORT_BLOCK_START: u16 = 20_000;
pub const PORT_BLOCK_SIZE: u16 = 16;
pub const PORT_BLOCK_COUNT: u16 = 1_000;
pub const DEFAULT_GUEST_PORTS: [u16; 8] = [22, 3000, 5173, 5432, 6379, 8000, 8080, 8081];

#[derive(Clone, Debug)]
pub struct PortMapping {
    pub host: u16,
    pub guest: u16,
}

pub fn choose_port_block(
    identity: &str,
    reserved_host_base: Option<u16>,
    used_host_bases: &[u16],
) -> Result<u16, String> {
    if let Some(base) = reserved_host_base {
        return Ok(base);
    }

    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    let seed = (hasher.finish() % u64::from(PORT_BLOCK_COUNT)) as u16;

    for offset in 0..PORT_BLOCK_COUNT {
        let slot = (seed + offset) % PORT_BLOCK_COUNT;
        let base = PORT_BLOCK_START + (slot * PORT_BLOCK_SIZE);
        if !used_host_bases.contains(&base) {
            return Ok(base);
        }
    }

    Err("no free host port block available".to_string())
}

pub fn build_port_mappings(base: u16, guest_ports: &[u16]) -> Vec<PortMapping> {
    guest_ports
        .iter()
        .enumerate()
        .map(|(index, guest)| PortMapping {
            host: base + index as u16,
            guest: *guest,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{PORT_BLOCK_SIZE, build_port_mappings, choose_port_block};

    #[test]
    fn port_block_is_stable() {
        let first = choose_port_block("repo|branch", None, &[]).unwrap();
        let second = choose_port_block("repo|branch", None, &[]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn reserved_port_block_is_preserved() {
        let reserved = choose_port_block("repo|branch", None, &[]).unwrap();
        let reused = choose_port_block("different", Some(reserved), &[]).unwrap();
        assert_eq!(reserved, reused);
    }

    #[test]
    fn used_blocks_are_skipped() {
        let first = choose_port_block("repo|branch", None, &[]).unwrap();
        let second = choose_port_block("repo|branch", None, &[first]).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn host_ports_increment_within_block() {
        let mappings = build_port_mappings(24_000, &[22, 3000, 8080]);
        assert_eq!(mappings[0].host, 24_000);
        assert_eq!(mappings[1].host, 24_001);
        assert_eq!(mappings[2].host, 24_002);
        assert!(PORT_BLOCK_SIZE >= mappings.len() as u16);
    }
}
