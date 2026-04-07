use std::collections::HashSet;
use std::fs;
use std::sync::Mutex;

static BLOCKED_IPS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn load_blocked_ips(filepath: &str) {
    let mut blocked = HashSet::new();

    // Try to read the file, if it doesn't exist that's OK
    match fs::read_to_string(filepath) {
        Ok(content) => {
            for line in content.lines() {
                let trimmed = line.trim();
                // Skip empty lines and comments
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                blocked.insert(trimmed.to_string());
            }
            log::info!("Loaded {} blocked IP(s) from {}", blocked.len(), filepath);
        }
        Err(e) => {
            // File doesn't exist or can't be read - that's OK, no IPs blocked
            log::info!("No robots.txt found or cannot read it: {} (IP blocking disabled)", e);
        }
    }

    let mut store = BLOCKED_IPS.lock().unwrap();
    *store = Some(blocked);
}

pub fn is_ip_blocked(ip: &str) -> bool {
    let store = BLOCKED_IPS.lock().unwrap();
    if let Some(ref blocked) = *store {
        blocked.contains(ip)
    } else {
        false
    }
}
