use hostname::get as get_hostname;
use sysinfo::System;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub architecture: String,
    pub hostname: String,
    pub is_virtualized: bool,
}

pub fn get_system_info() -> SystemInfo {
    let hostname = get_hostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    let os_name = System::name().unwrap_or_else(|| "Unknown OS".to_string());
    let os_version = System::os_version().unwrap_or_else(|| "Unknown Version".to_string());
    let architecture = if cfg!(target_arch = "x86_64") {
        "64-bit"
    } else if cfg!(target_arch = "x86") {
        "32-bit"
    } else if cfg!(target_arch = "aarch64") {
        "ARM64"
    } else if cfg!(target_arch = "arm") {
        "ARM32"
    } else {
        "Unknown"
    };

    let is_virtualized = get_is_virtualized();

    SystemInfo {
        os_name,
        os_version,
        architecture: architecture.to_string(),
        hostname,
        is_virtualized,
    }
}

#[cfg(target_os = "linux")]
pub fn get_is_virtualized() -> bool {
    crate::linux::is_virtualized()
}

#[cfg(target_os = "windows")]
pub fn get_is_virtualized() -> bool {
    crate::windows::is_virtualized()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn get_is_virtualized() -> bool {
    false
}
