use std::env;
use std::fs;
use std::path::Path;

pub const CONFIG_PATH: &str = "config.yaml";
pub const CONFIG_PATH_ENV: &str = "DISKMON_CONFIG_PATH";

pub fn config_path() -> String {
    env::var(CONFIG_PATH_ENV).unwrap_or_else(|_| CONFIG_PATH.to_string())
}

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    pub mail_enabled: bool,
    pub smtp_server: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub email_from: String,
    pub email_to: String,
    pub smtp_security: Option<String>,  // "none", "starttls", "ssl"
    pub threshold_percent: Option<f64>, // Alert threshold as used/occupied percentage
    pub warning_threshold_percent: Option<f64>, // Warning severity threshold as used percentage
    pub critical_threshold_percent: Option<f64>, // Critical severity threshold as used percentage
    pub emergency_threshold_percent: Option<f64>, // Emergency severity threshold as used percentage
    pub alert_cooldown_hours: Option<u64>, // Reminder interval while an alert remains active
    pub recovery_threshold_percent: Option<f64>, // Alert is cleared when used percentage drops to this value
    pub send_recovery_email: Option<bool>,       // Send a notification when an alert recovers
    pub debug: Option<bool>,                     // Enable debug output
    pub friendly_name: Option<String>,           // New: single friendly name
    pub excluded_disks: Option<Vec<String>>, // List of disks to exclude (drive letters or device names)
    pub archive_scan_enabled: Option<bool>,  // Include old archive candidates in email reports
    pub archive_scan_path: Option<String>,   // Local ArcSight archive directory to scan
    pub archive_scan_min_age_days: Option<i64>, // Minimum archive age in days for manual move suggestions
    pub archive_scan_limit: Option<usize>,      // Maximum archive candidates to include in email
    pub archive_scan_require_supplement_pair: Option<bool>, // Require DDMMYYYY + DDMMYYYY.suppliment pair
}

pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Config, String> {
    // Check if config file exists
    if !path.as_ref().exists() {
        return Err(format!(
            "Configuration file not found: {}",
            path.as_ref().display()
        ));
    }

    // Check file permissions on Unix systems
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&path) {
            let permissions = metadata.permissions();
            let mode = permissions.mode();
            // Check if file is readable by group or others (world-readable)
            if mode & 0o044 != 0 {
                eprintln!(
                    "[SECURITY WARNING] Configuration file {} has overly permissive permissions (readable by group/others). Consider: chmod 600 {}",
                    path.as_ref().display(),
                    path.as_ref().display()
                );
            }
        }
    }

    // Read config file
    let data = fs::read_to_string(path).map_err(|e| format!("Failed to read config file: {e}"))?;

    // Parse YAML
    let config: Config =
        serde_yaml::from_str(&data).map_err(|e| format!("Failed to parse config YAML: {e}"))?;

    // Validate required fields
    validate_config(&config)?;

    // Apply environment variable overrides for sensitive data
    let config = apply_env_overrides(config);

    Ok(config)
}

fn apply_env_overrides(mut config: Config) -> Config {
    // Override SMTP credentials from environment variables if available
    if let Ok(smtp_user) = env::var("DISKMON_SMTP_USER") {
        if !smtp_user.trim().is_empty() {
            config.smtp_user = smtp_user;
        }
    }

    if let Ok(smtp_pass) = env::var("DISKMON_SMTP_PASS") {
        if !smtp_pass.trim().is_empty() {
            config.smtp_pass = smtp_pass;
        }
    }

    if let Ok(email_from) = env::var("DISKMON_EMAIL_FROM") {
        if !email_from.trim().is_empty() {
            config.email_from = email_from;
        }
    }

    if let Ok(email_to) = env::var("DISKMON_EMAIL_TO") {
        if !email_to.trim().is_empty() {
            config.email_to = email_to;
        }
    }

    config
}

fn validate_config(config: &Config) -> Result<(), String> {
    let mut missing_keys = Vec::new();
    let mut warnings = Vec::new();

    // Check for empty required string fields (except smtp_user and smtp_pass)
    if config.smtp_server.trim().is_empty() {
        missing_keys.push("smtp_server");
    }
    if config.email_from.trim().is_empty() {
        missing_keys.push("email_from");
    }
    if config.email_to.trim().is_empty() {
        missing_keys.push("email_to");
    }

    // Check port is valid
    if config.smtp_port == 0 {
        missing_keys.push("smtp_port (must be 1-65535)");
    }

    // Validate threshold_percent if provided. It now means occupied/used percentage.
    if let Some(threshold) = config.threshold_percent {
        if threshold < 1.0 || threshold > 100.0 {
            missing_keys.push("threshold_percent (must be between 1.0 and 100.0)");
        }
    }

    for (name, threshold) in [
        (
            "warning_threshold_percent",
            config.warning_threshold_percent,
        ),
        (
            "critical_threshold_percent",
            config.critical_threshold_percent,
        ),
        (
            "emergency_threshold_percent",
            config.emergency_threshold_percent,
        ),
    ] {
        if let Some(threshold) = threshold {
            if threshold < 1.0 || threshold > 100.0 {
                missing_keys.push(match name {
                    "warning_threshold_percent" => {
                        "warning_threshold_percent (must be between 1.0 and 100.0)"
                    }
                    "critical_threshold_percent" => {
                        "critical_threshold_percent (must be between 1.0 and 100.0)"
                    }
                    _ => "emergency_threshold_percent (must be between 1.0 and 100.0)",
                });
            }
        }
    }

    let warning_threshold = config
        .warning_threshold_percent
        .or(config.threshold_percent)
        .unwrap_or(85.0);
    let critical_threshold = config.critical_threshold_percent.unwrap_or(90.0);
    let emergency_threshold = config.emergency_threshold_percent.unwrap_or(95.0);
    if !(warning_threshold < critical_threshold && critical_threshold < emergency_threshold) {
        missing_keys.push("severity thresholds must be ordered: warning < critical < emergency");
    }

    if let Some(cooldown) = config.alert_cooldown_hours {
        if cooldown == 0 {
            missing_keys.push("alert_cooldown_hours (must be greater than 0)");
        }
    }

    if let Some(recovery_threshold) = config.recovery_threshold_percent {
        if recovery_threshold < 0.0 || recovery_threshold > 100.0 {
            missing_keys.push("recovery_threshold_percent (must be between 0.0 and 100.0)");
        }
        if recovery_threshold >= warning_threshold {
            missing_keys
                .push("recovery_threshold_percent (must be lower than warning alert threshold)");
        }
    }

    // Validate smtp_security
    if let Some(ref sec) = config.smtp_security {
        let sec = sec.to_lowercase();
        if sec != "none" && sec != "starttls" && sec != "ssl" {
            missing_keys.push("smtp_security (must be one of: none, starttls, ssl)");
        }
        if sec == "none" {
            warnings.push(
                "SMTP security is set to 'none'. This is insecure and not recommended.".to_string(),
            );
        }
    }

    // Validate email addresses (basic check)
    if !config.email_from.contains('@') {
        missing_keys.push("email_from (must be a valid email address)");
    }

    // Validate email_to: allow comma-separated recipients in a single string
    let mut email_to_count = 0usize;
    let mut email_to_invalid = false;
    for addr in config.email_to.split(',') {
        let a = addr.trim();
        if a.is_empty() {
            continue;
        }
        email_to_count += 1;
        if !a.contains('@') {
            email_to_invalid = true;
        }
    }
    if email_to_count == 0 {
        missing_keys.push("email_to (must be a valid email address)");
    }
    if email_to_invalid {
        missing_keys.push("email_to (one or more recipients appear invalid)");
    }

    // Warn if debug is enabled
    if config.debug.unwrap_or(false) {
        warnings.push(
            "Debug mode is enabled. This may expose sensitive information in logs.".to_string(),
        );
    }

    // Validate excluded_disks
    if let Some(ref excluded) = config.excluded_disks {
        for disk in excluded {
            if disk.trim().is_empty() {
                continue; // Ignore empty values
            }
            if cfg!(windows) {
                // Should be like "C:", "D:", etc.
                if !(disk.len() == 2 && disk.chars().nth(1) == Some(':')) {
                    warnings.push(format!(
                        "Invalid excluded disk '{}': must be a drive letter like 'C:'",
                        disk
                    ));
                }
            } else {
                // Should be like "sda", "nvme0n1", etc.
                if disk.contains('/') || disk.is_empty() {
                    warnings.push(format!(
                        "Invalid excluded disk '{}': must be a device name like 'sda' or 'nvme0n1'",
                        disk
                    ));
                }
            }
        }
    }

    if config.archive_scan_enabled.unwrap_or(false) {
        match config.archive_scan_path.as_deref() {
            Some(path) if !path.trim().is_empty() => {}
            _ => {
                missing_keys.push("archive_scan_path (required when archive_scan_enabled is true)")
            }
        }

        if let Some(days) = config.archive_scan_min_age_days {
            if days < 0 {
                missing_keys.push("archive_scan_min_age_days (must be 0 or greater)");
            }
        }

        if let Some(limit) = config.archive_scan_limit {
            if limit == 0 {
                missing_keys.push("archive_scan_limit (must be greater than 0)");
            }
        }
    }

    if !missing_keys.is_empty() {
        return Err(format!(
            "Missing or invalid required configuration keys: {}",
            missing_keys.join(", ")
        ));
    }
    if !warnings.is_empty() {
        eprintln!("[CONFIG WARNING] {}", &warnings.join(" | "));
    }
    Ok(())
}
