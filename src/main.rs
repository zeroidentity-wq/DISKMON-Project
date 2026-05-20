use backoff::{ExponentialBackoff, backoff::Backoff};
use chrono::{DateTime, Local, Utc};
use clap::Parser;
use colored::*;
use lettre::{
    Message, SmtpTransport, Transport,
    message::header::ContentType,
    transport::smtp::{authentication::Credentials, client::Tls, client::TlsParameters},
};
use log::{debug, error, warn};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Duration;

mod config;
mod system;

const ALERT_STATE_PATH: &str = "diskmon-state.json";
const ALERT_STATE_PATH_ENV: &str = "DISKMON_STATE_PATH";
const HISTORY_RETENTION_DAYS: i64 = 31;

#[cfg(target_os = "linux")]
pub mod linux;

#[derive(Parser)]
#[command(name = "diskmon-mail-v2")]
#[command(about = "Monitorizeaza spatiul de stocare si trimite alerte email")]
#[command(version)]
struct Cli {
    /// Trimite raport complet indiferent de prag. Util pentru raport zilnic sau verificare la cerere.
    #[arg(long)]
    force_mail: bool,

    /// Afiseaza rezultatul in format JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DiskInfo {
    mount_point: String,
    display_name: String,
    file_system: String,
    total_space: u64,
    used_space: u64,
    available_space: u64,
    used_percent: f64,
    free_percent: f64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct AlertStateFile {
    #[serde(default)]
    alerts: BTreeMap<String, MountAlertState>,
    #[serde(default)]
    history: BTreeMap<String, Vec<DiskHistorySample>>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct MountAlertState {
    active: bool,
    first_alert_at: Option<DateTime<Utc>>,
    last_notification_at: Option<DateTime<Utc>>,
    last_seen_percent: f64,
    #[serde(default)]
    last_severity: Option<AlertSeverity>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct DiskHistorySample {
    timestamp: DateTime<Utc>,
    total_space: u64,
    used_space: u64,
    available_space: u64,
    used_percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportKind {
    Forced,
    Alert,
    Reminder,
    Recovery,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize,
)]
enum AlertSeverity {
    Warning,
    Critical,
    Emergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
enum AlertEventKind {
    Alert,
    Escalation,
    Reminder,
    Recovery,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AlertEvent {
    mount_key: String,
    mount_point: String,
    display_name: String,
    used_percent: f64,
    severity: Option<AlertSeverity>,
    kind: AlertEventKind,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
struct AlertThresholds {
    warning: f64,
    critical: f64,
    emergency: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TrendInfo {
    growth_24h: Option<i64>,
    growth_7d: Option<i64>,
    growth_30d: Option<i64>,
    time_to_full_seconds: Option<i64>,
    abnormal_growth: bool,
}

#[derive(Debug, Clone)]
struct NotificationDecision {
    report_kind: Option<ReportKind>,
    events: Vec<AlertEvent>,
    state: AlertStateFile,
    state_changed: bool,
}

fn supports_colors() -> bool {
    std::env::var("TERM").is_ok() && cfg!(unix)
}

fn init_colors() {
    if !supports_colors() {
        colored::control::set_override(false);
    }
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn signed_bytes_to_gb(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn format_growth(value: Option<i64>) -> String {
    match value {
        Some(bytes) if bytes >= 0 => format!("+{:.2} GB", signed_bytes_to_gb(bytes)),
        Some(bytes) => format!("{:.2} GB", signed_bytes_to_gb(bytes)),
        None => "N/A".to_string(),
    }
}

fn format_time_to_full(seconds: Option<i64>) -> String {
    match seconds {
        Some(seconds) if seconds <= 0 => "acum".to_string(),
        Some(seconds) => {
            let days = seconds as f64 / 86_400.0;
            if days >= 2.0 {
                format!("{:.0} zile", days.ceil())
            } else {
                let hours = seconds as f64 / 3_600.0;
                if hours >= 2.0 {
                    format!("{:.0} ore", hours.ceil())
                } else {
                    format!("{:.0} minute", (seconds as f64 / 60.0).ceil().max(1.0))
                }
            }
        }
        None => "N/A".to_string(),
    }
}

fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn default_recovery_threshold(threshold: f64) -> f64 {
    threshold
}

fn alert_thresholds(cfg: &config::Config) -> AlertThresholds {
    AlertThresholds {
        warning: cfg
            .warning_threshold_percent
            .or(cfg.threshold_percent)
            .unwrap_or(85.0),
        critical: cfg.critical_threshold_percent.unwrap_or(90.0),
        emergency: cfg.emergency_threshold_percent.unwrap_or(95.0),
    }
}

fn alert_cooldown(cfg: &config::Config) -> chrono::Duration {
    chrono::Duration::hours(cfg.alert_cooldown_hours.unwrap_or(12) as i64)
}

fn recovery_threshold(cfg: &config::Config, threshold: f64) -> f64 {
    cfg.recovery_threshold_percent
        .unwrap_or_else(|| default_recovery_threshold(threshold))
}

fn send_recovery_email(cfg: &config::Config) -> bool {
    cfg.send_recovery_email.unwrap_or(true)
}

fn severity_for_percent(used_percent: f64, thresholds: AlertThresholds) -> Option<AlertSeverity> {
    if used_percent >= thresholds.emergency {
        Some(AlertSeverity::Emergency)
    } else if used_percent >= thresholds.critical {
        Some(AlertSeverity::Critical)
    } else if used_percent >= thresholds.warning {
        Some(AlertSeverity::Warning)
    } else {
        None
    }
}

fn severity_label(severity: AlertSeverity) -> &'static str {
    match severity {
        AlertSeverity::Warning => "WARNING",
        AlertSeverity::Critical => "CRITICAL",
        AlertSeverity::Emergency => "EMERGENCY",
    }
}

fn severity_color(severity: AlertSeverity) -> &'static str {
    match severity {
        AlertSeverity::Warning => "#b54708",
        AlertSeverity::Critical => "#b42318",
        AlertSeverity::Emergency => "#7a271a",
    }
}

fn severity_row_color(severity: AlertSeverity) -> &'static str {
    match severity {
        AlertSeverity::Warning => "#fef3c7",
        AlertSeverity::Critical => "#fee2e2",
        AlertSeverity::Emergency => "#fecaca",
    }
}

fn severity_html(severity: Option<AlertSeverity>) -> String {
    match severity {
        Some(AlertSeverity::Warning) => "<span class=\"text-warning\">WARNING</span>".to_string(),
        Some(AlertSeverity::Critical) => {
            "<span class=\"text-critical\">CRITICAL</span>".to_string()
        }
        Some(AlertSeverity::Emergency) => {
            "<span class=\"text-emergency\">EMERGENCY</span>".to_string()
        }
        None => "<span class=\"text-ok\">OK</span>".to_string(),
    }
}

fn mount_key(disk: &DiskInfo) -> String {
    format!("{}|{}", disk.mount_point, disk.file_system)
}

fn empty_alert_state() -> AlertStateFile {
    AlertStateFile {
        alerts: BTreeMap::new(),
        history: BTreeMap::new(),
    }
}

fn alert_state_path() -> String {
    std::env::var(ALERT_STATE_PATH_ENV).unwrap_or_else(|_| ALERT_STATE_PATH.to_string())
}

fn load_alert_state(debug_enabled: bool) -> AlertStateFile {
    let path = alert_state_path();
    match fs::read_to_string(&path) {
        Ok(data) => match serde_json::from_str(&data) {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    "Nu s-a putut parsa {}: {}. Pornesc cu stare goala.",
                    path, e
                );
                empty_alert_state()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => empty_alert_state(),
        Err(e) => {
            if debug_enabled {
                debug!("Nu s-a putut citi {}: {}", path, e);
            }
            empty_alert_state()
        }
    }
}

fn save_alert_state(state: &AlertStateFile) -> Result<(), String> {
    let path = alert_state_path();
    let data = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Nu s-a putut serializa starea alertelor: {e}"))?;
    fs::write(&path, data).map_err(|e| format!("Nu s-a putut scrie {}: {}", path, e))
}

fn disk_history_sample(disk: &DiskInfo, now: DateTime<Utc>) -> DiskHistorySample {
    DiskHistorySample {
        timestamp: now,
        total_space: disk.total_space,
        used_space: disk.used_space,
        available_space: disk.available_space,
        used_percent: disk.used_percent,
    }
}

fn sample_for_window<'a>(
    samples: &'a [DiskHistorySample],
    now: DateTime<Utc>,
    window: chrono::Duration,
) -> Option<&'a DiskHistorySample> {
    let cutoff = now - window;
    let minimum_age = window / 2;
    samples
        .iter()
        .filter(|sample| sample.timestamp <= cutoff)
        .max_by_key(|sample| sample.timestamp)
        .or_else(|| {
            samples
                .iter()
                .filter(|sample| now.signed_duration_since(sample.timestamp) >= minimum_age)
                .min_by_key(|sample| sample.timestamp)
        })
}

fn growth_for_window(
    samples: &[DiskHistorySample],
    disk: &DiskInfo,
    now: DateTime<Utc>,
    window: chrono::Duration,
) -> Option<i64> {
    sample_for_window(samples, now, window)
        .map(|sample| disk.used_space as i64 - sample.used_space as i64)
}

fn elapsed_since_sample(
    samples: &[DiskHistorySample],
    now: DateTime<Utc>,
    window: chrono::Duration,
) -> Option<chrono::Duration> {
    sample_for_window(samples, now, window)
        .map(|sample| now.signed_duration_since(sample.timestamp))
}

fn estimate_time_to_full(
    samples: &[DiskHistorySample],
    disk: &DiskInfo,
    now: DateTime<Utc>,
) -> Option<i64> {
    let windows = [
        chrono::Duration::hours(24),
        chrono::Duration::days(7),
        chrono::Duration::days(30),
    ];

    for window in windows {
        let Some(sample) = sample_for_window(samples, now, window) else {
            continue;
        };
        let elapsed = now.signed_duration_since(sample.timestamp).num_seconds();
        let growth = disk.used_space as i64 - sample.used_space as i64;
        if elapsed <= 0 || growth <= 0 {
            continue;
        }
        let bytes_per_second = growth as f64 / elapsed as f64;
        if bytes_per_second <= 0.0 {
            continue;
        }
        return Some((disk.available_space as f64 / bytes_per_second).ceil() as i64);
    }

    None
}

fn is_abnormal_growth(
    samples: &[DiskHistorySample],
    _disk: &DiskInfo,
    now: DateTime<Utc>,
    growth_24h: Option<i64>,
    growth_7d: Option<i64>,
) -> bool {
    let Some(growth_24h) = growth_24h else {
        return false;
    };
    if growth_24h <= 0 {
        return false;
    }
    let Some(growth_7d) = growth_7d else {
        return false;
    };
    if growth_7d <= 0 {
        return false;
    }
    let Some(elapsed_7d) = elapsed_since_sample(samples, now, chrono::Duration::days(7)) else {
        return false;
    };
    let elapsed_days = elapsed_7d.num_seconds() as f64 / 86_400.0;
    if elapsed_days < 2.0 {
        return false;
    }
    let average_daily_growth = growth_7d as f64 / elapsed_days;
    let one_gb = 1024_i64 * 1024 * 1024;
    growth_24h > one_gb && growth_24h as f64 > average_daily_growth * 2.0
}

fn calculate_trends(
    state: &AlertStateFile,
    disks: &[DiskInfo],
    now: DateTime<Utc>,
) -> BTreeMap<String, TrendInfo> {
    let mut trends = BTreeMap::new();

    for disk in disks {
        let key = mount_key(disk);
        let samples = state.history.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let growth_24h = growth_for_window(samples, disk, now, chrono::Duration::hours(24));
        let growth_7d = growth_for_window(samples, disk, now, chrono::Duration::days(7));
        let growth_30d = growth_for_window(samples, disk, now, chrono::Duration::days(30));
        let time_to_full_seconds = estimate_time_to_full(samples, disk, now);
        let abnormal_growth = is_abnormal_growth(samples, disk, now, growth_24h, growth_7d);

        trends.insert(
            key,
            TrendInfo {
                growth_24h,
                growth_7d,
                growth_30d,
                time_to_full_seconds,
                abnormal_growth,
            },
        );
    }

    trends
}

fn update_history(state: &mut AlertStateFile, disks: &[DiskInfo], now: DateTime<Utc>) {
    let retention_cutoff = now - chrono::Duration::days(HISTORY_RETENTION_DAYS);
    let seen_keys: BTreeSet<String> = disks.iter().map(mount_key).collect();

    for disk in disks {
        let key = mount_key(disk);
        let samples = state.history.entry(key).or_default();
        samples.push(disk_history_sample(disk, now));
        samples.retain(|sample| sample.timestamp >= retention_cutoff);
        samples.sort_by_key(|sample| sample.timestamp);
    }

    state.history.retain(|key, samples| {
        samples.retain(|sample| sample.timestamp >= retention_cutoff);
        seen_keys.contains(key) || !samples.is_empty()
    });
}

fn pseudo_filesystems() -> &'static [&'static str] {
    &[
        "tmpfs",
        "devtmpfs",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "overlay",
        "squashfs",
        "securityfs",
        "rpc_pipefs",
        "fusectl",
        "mqueue",
        "hugetlbfs",
        "autofs",
        "binfmt_misc",
        "debugfs",
        "tracefs",
        "configfs",
        "pstore",
        "efivarfs",
        "nsfs",
    ]
}

fn is_excluded(cfg: &config::Config, mount_point: &str, filesystem: &str) -> bool {
    cfg.excluded_disks
        .as_ref()
        .map(|excluded| {
            excluded.iter().any(|item| {
                let item = item.trim();
                if item.is_empty() {
                    return false;
                }
                let needle = item.to_uppercase();
                mount_point.to_uppercase().contains(&needle)
                    || filesystem.to_uppercase().contains(&needle)
            })
        })
        .unwrap_or(false)
}

fn disk_from_df_parts(
    cfg: &config::Config,
    filesystem: &str,
    fstype: &str,
    total_k: &str,
    used_k: &str,
    available_k: &str,
    mount_point: &str,
) -> Option<DiskInfo> {
    if pseudo_filesystems().contains(&fstype) || is_excluded(cfg, mount_point, filesystem) {
        return None;
    }

    let total = total_k.parse::<u64>().ok()?.saturating_mul(1024);
    let used = used_k.parse::<u64>().ok()?.saturating_mul(1024);
    let available = available_k.parse::<u64>().ok()?.saturating_mul(1024);
    if total == 0 {
        return None;
    }

    Some(DiskInfo {
        mount_point: mount_point.to_string(),
        display_name: format!("{} ({})", mount_point, filesystem),
        file_system: fstype.to_string(),
        total_space: total,
        used_space: used.min(total),
        available_space: available.min(total),
        used_percent: percent(used.min(total), total),
        free_percent: percent(available.min(total), total),
    })
}

fn collect_disks_from_df(cfg: &config::Config, debug_enabled: bool) -> Vec<DiskInfo> {
    let mut disks = Vec::new();

    if let Ok(output) = std::process::Command::new("df")
        .arg("-T")
        .arg("-P")
        .output()
    {
        if output.status.success() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                for (index, line) in text.lines().enumerate() {
                    if index == 0 {
                        continue;
                    }
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() < 7 {
                        continue;
                    }
                    let mount_point = parts[6..].join(" ");
                    if let Some(disk) = disk_from_df_parts(
                        cfg,
                        parts[0],
                        parts[1],
                        parts[2],
                        parts[3],
                        parts[4],
                        &mount_point,
                    ) {
                        disks.push(disk);
                    }
                }
            }
        }
    }

    if !disks.is_empty() {
        return disks;
    }

    if debug_enabled {
        debug!("df -T -P did not return usable data; falling back to df -k -P");
    }

    if let Ok(output) = std::process::Command::new("df")
        .arg("-k")
        .arg("-P")
        .output()
    {
        if output.status.success() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                for (index, line) in text.lines().enumerate() {
                    if index == 0 {
                        continue;
                    }
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() < 6 {
                        continue;
                    }
                    let filesystem = parts[0];
                    let mount_point = parts[5..].join(" ");
                    if filesystem.starts_with("tmpfs")
                        || filesystem.starts_with("devtmpfs")
                        || mount_point.starts_with("/sys")
                        || mount_point.starts_with("/proc")
                        || mount_point.starts_with("/dev")
                    {
                        continue;
                    }
                    if let Some(disk) = disk_from_df_parts(
                        cfg,
                        filesystem,
                        "necunoscut",
                        parts[1],
                        parts[2],
                        parts[3],
                        &mount_point,
                    ) {
                        disks.push(disk);
                    }
                }
            }
        }
    }

    disks
}

fn collect_disks_from_sysinfo(cfg: &config::Config) -> Vec<DiskInfo> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter_map(|disk| {
            let mount_point = disk.mount_point().to_str()?.to_string();
            let filesystem_name = disk.name().to_str().unwrap_or("");
            if is_excluded(cfg, &mount_point, filesystem_name) {
                return None;
            }

            let total = disk.total_space();
            let available = disk.available_space();
            if total == 0 {
                return None;
            }
            let used = total.saturating_sub(available);

            Some(DiskInfo {
                display_name: format!("{} ({})", mount_point, filesystem_name),
                mount_point,
                file_system: disk
                    .file_system()
                    .to_str()
                    .unwrap_or("necunoscut")
                    .to_string(),
                total_space: total,
                used_space: used,
                available_space: available,
                used_percent: percent(used, total),
                free_percent: percent(available, total),
            })
        })
        .collect()
}

fn get_monitored_disks(cfg: &config::Config, debug_enabled: bool) -> Vec<DiskInfo> {
    let mut disks = if cfg!(unix) {
        collect_disks_from_df(cfg, debug_enabled)
    } else {
        Vec::new()
    };

    if disks.is_empty() {
        disks = collect_disks_from_sysinfo(cfg);
    }

    if debug_enabled {
        for disk in &disks {
            debug!(
                "Disk monitorizat: mount={}, fs={}, total={}, used={}, available={}, used_percent={:.2}",
                disk.mount_point,
                disk.file_system,
                disk.total_space,
                disk.used_space,
                disk.available_space,
                disk.used_percent
            );
        }
    }

    disks
}

fn alert_disks(disks: &[DiskInfo], thresholds: AlertThresholds) -> Vec<&DiskInfo> {
    disks
        .iter()
        .filter(|disk| severity_for_percent(disk.used_percent, thresholds).is_some())
        .collect()
}

fn choose_report_kind(events: &[AlertEvent]) -> Option<ReportKind> {
    if events.iter().any(|event| {
        event.kind == AlertEventKind::Alert || event.kind == AlertEventKind::Escalation
    }) {
        Some(ReportKind::Alert)
    } else if events
        .iter()
        .any(|event| event.kind == AlertEventKind::Reminder)
    {
        Some(ReportKind::Reminder)
    } else if events
        .iter()
        .any(|event| event.kind == AlertEventKind::Recovery)
    {
        Some(ReportKind::Recovery)
    } else {
        None
    }
}

fn event_label(kind: AlertEventKind) -> &'static str {
    match kind {
        AlertEventKind::Alert => "Alerta noua",
        AlertEventKind::Escalation => "Escaladare",
        AlertEventKind::Reminder => "Reminder",
        AlertEventKind::Recovery => "Revenire",
    }
}

fn subject_prefix(
    report_kind: ReportKind,
    disks: &[DiskInfo],
    thresholds: AlertThresholds,
) -> &'static str {
    match report_kind {
        ReportKind::Alert => "🔴 [ALARMĂ]",
        ReportKind::Reminder => "🔴 [REMINDER ALARMĂ]",
        ReportKind::Recovery => "🟢 [REVENIRE OK]",
        ReportKind::Forced => {
            if alert_disks(disks, thresholds).is_empty() {
                "🟢 [RAPORT OK]"
            } else {
                "🔴 [RAPORT ALERTĂ]"
            }
        }
    }
}

fn overall_status(
    disks: &[DiskInfo],
    thresholds: AlertThresholds,
    trends: &BTreeMap<String, TrendInfo>,
) -> &'static str {
    if disks.iter().any(|disk| {
        severity_for_percent(disk.used_percent, thresholds) == Some(AlertSeverity::Emergency)
    }) {
        "EMERGENCY"
    } else if disks.iter().any(|disk| {
        severity_for_percent(disk.used_percent, thresholds) == Some(AlertSeverity::Critical)
    }) {
        "CRITICAL"
    } else if disks.iter().any(|disk| {
        severity_for_percent(disk.used_percent, thresholds) == Some(AlertSeverity::Warning)
    }) {
        "WARNING"
    } else if trends.values().any(|trend| trend.abnormal_growth) {
        "ATENTIE TREND"
    } else {
        "OK"
    }
}

fn severity_counts(
    disks: &[DiskInfo],
    thresholds: AlertThresholds,
) -> (usize, usize, usize, usize) {
    let mut ok = 0;
    let mut warning = 0;
    let mut critical = 0;
    let mut emergency = 0;

    for disk in disks {
        match severity_for_percent(disk.used_percent, thresholds) {
            Some(AlertSeverity::Warning) => warning += 1,
            Some(AlertSeverity::Critical) => critical += 1,
            Some(AlertSeverity::Emergency) => emergency += 1,
            None => ok += 1,
        }
    }

    (ok, warning, critical, emergency)
}

fn executive_table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    if rows.is_empty() {
        return "<p class=\"empty\">Nu exista date suficiente.</p>".to_string();
    }

    let header_html = headers
        .iter()
        .map(|header| format!("<th>{}</th>", escape_html(header)))
        .collect::<Vec<_>>()
        .join("");
    let row_html = rows
        .into_iter()
        .map(|row| {
            let cells = row
                .into_iter()
                .map(|cell| format!("<td>{}</td>", cell))
                .collect::<Vec<_>>()
                .join("");
            format!("<tr>{}</tr>", cells)
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<table class=\"mini\"><thead><tr>{}</tr></thead><tbody>{}</tbody></table>",
        header_html, row_html
    )
}

fn build_executive_summary(
    disks: &[DiskInfo],
    thresholds: AlertThresholds,
    trends: &BTreeMap<String, TrendInfo>,
) -> String {
    let (ok_count, warning_count, critical_count, emergency_count) =
        severity_counts(disks, thresholds);
    let status = overall_status(disks, thresholds, trends);

    let mut top_occupied = disks.iter().collect::<Vec<_>>();
    top_occupied.sort_by(|left, right| {
        right
            .used_percent
            .partial_cmp(&left.used_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_occupied_rows = top_occupied
        .into_iter()
        .take(5)
        .map(|disk| {
            let severity = severity_for_percent(disk.used_percent, thresholds);
            vec![
                escape_html(&disk.mount_point),
                format!("{:.2}%", disk.used_percent),
                format!("{:.2} GB", bytes_to_gb(disk.available_space)),
                severity_html(severity),
            ]
        })
        .collect::<Vec<_>>();

    let mut top_growth = disks
        .iter()
        .filter_map(|disk| {
            trends
                .get(&mount_key(disk))
                .and_then(|trend| trend.growth_24h.map(|growth| (disk, trend, growth)))
        })
        .filter(|(_, _, growth)| *growth > 0)
        .collect::<Vec<_>>();
    top_growth.sort_by(|(_, _, left), (_, _, right)| right.cmp(left));
    let top_growth_rows = top_growth
        .into_iter()
        .take(5)
        .map(|(disk, trend, growth)| {
            vec![
                escape_html(&disk.mount_point),
                format_growth(Some(growth)),
                format_growth(trend.growth_7d),
                if trend.abnormal_growth {
                    "Da".to_string()
                } else {
                    "Nu".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();

    let mut time_to_full = disks
        .iter()
        .filter_map(|disk| {
            trends
                .get(&mount_key(disk))
                .and_then(|trend| trend.time_to_full_seconds.map(|seconds| (disk, seconds)))
        })
        .collect::<Vec<_>>();
    time_to_full.sort_by_key(|(_, seconds)| *seconds);
    let time_to_full_rows = time_to_full
        .into_iter()
        .take(5)
        .map(|(disk, seconds)| {
            vec![
                escape_html(&disk.mount_point),
                format_time_to_full(Some(seconds)),
                format!("{:.2} GB", bytes_to_gb(disk.available_space)),
                severity_html(severity_for_percent(disk.used_percent, thresholds)),
            ]
        })
        .collect::<Vec<_>>();

    let mut recommendations = Vec::new();
    if emergency_count > 0 {
        recommendations.push("Interventie imediata pe mount point-urile EMERGENCY: curatare, extindere volum sau mutare date.".to_string());
    }
    if critical_count > 0 {
        recommendations
            .push("Plan de remediere in aceeasi zi pentru mount point-urile CRITICAL.".to_string());
    }
    if warning_count > 0 {
        recommendations
            .push("Urmarire si verificare cauze pentru mount point-urile WARNING.".to_string());
    }
    if trends.values().any(|trend| trend.abnormal_growth) {
        recommendations.push(
            "Investigati procesele/logurile care au produs crestere anormala in ultimele 24h."
                .to_string(),
        );
    }
    if disks.iter().any(|disk| {
        trends
            .get(&mount_key(disk))
            .and_then(|trend| trend.time_to_full_seconds)
            .map(|seconds| seconds <= 7 * 86_400)
            .unwrap_or(false)
    }) {
        recommendations
            .push("Prioritizati mount point-urile cu estimare de umplere sub 7 zile.".to_string());
    }
    if recommendations.is_empty() {
        recommendations
            .push("Nu sunt actiuni urgente; continuati monitorizarea zilnica.".to_string());
    }
    let recommendations = recommendations
        .into_iter()
        .map(|item| format!("<li>{}</li>", escape_html(&item)))
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<div class="executive">
        <h2>Sumar executiv</h2>
        <div class="kpis">
          <div><span>Status general</span><strong>{status}</strong></div>
          <div><span>OK</span><strong>{ok_count}</strong></div>
          <div><span>WARNING</span><strong>{warning_count}</strong></div>
          <div><span>CRITICAL</span><strong>{critical_count}</strong></div>
          <div><span>EMERGENCY</span><strong>{emergency_count}</strong></div>
        </div>
        <h3>Top 5 cele mai ocupate mount point-uri</h3>
        {top_occupied}
        <h3>Top 5 cele mai mari cresteri fata de ziua precedenta</h3>
        {top_growth}
        <h3>Estimari time-to-full</h3>
        {time_to_full}
        <h3>Recomandari de actiune</h3>
        <ul class="recommendations">{recommendations}</ul>
      </div>"#,
        status = escape_html(status),
        ok_count = ok_count,
        warning_count = warning_count,
        critical_count = critical_count,
        emergency_count = emergency_count,
        top_occupied = executive_table(
            &["Mount point", "Ocupare", "Disponibil", "Severitate"],
            top_occupied_rows
        ),
        top_growth = executive_table(
            &["Mount point", "Crestere 24h", "Crestere 7 zile", "Anormal"],
            top_growth_rows
        ),
        time_to_full = executive_table(
            &["Mount point", "Estimare", "Disponibil", "Severitate"],
            time_to_full_rows
        ),
        recommendations = recommendations
    )
}

fn evaluate_alert_state(
    cfg: &config::Config,
    disks: &[DiskInfo],
    current_state: &AlertStateFile,
    thresholds: AlertThresholds,
    now: DateTime<Utc>,
) -> NotificationDecision {
    let cooldown = alert_cooldown(cfg);
    let recovery_threshold = recovery_threshold(cfg, thresholds.warning);
    let should_send_recovery = send_recovery_email(cfg);
    let mut next_state = current_state.clone();
    let mut events = Vec::new();
    let mut state_changed = false;
    let seen_keys: BTreeSet<String> = disks.iter().map(mount_key).collect();

    for disk in disks {
        let key = mount_key(disk);
        let previous = current_state.alerts.get(&key);
        let was_active = previous.map(|state| state.active).unwrap_or(false);
        let severity = severity_for_percent(disk.used_percent, thresholds);

        if let Some(severity) = severity {
            if !was_active {
                events.push(AlertEvent {
                    mount_key: key.clone(),
                    mount_point: disk.mount_point.clone(),
                    display_name: disk.display_name.clone(),
                    used_percent: disk.used_percent,
                    severity: Some(severity),
                    kind: AlertEventKind::Alert,
                });
                next_state.alerts.insert(
                    key,
                    MountAlertState {
                        active: true,
                        first_alert_at: Some(now),
                        last_notification_at: Some(now),
                        last_seen_percent: disk.used_percent,
                        last_severity: Some(severity),
                    },
                );
                state_changed = true;
            } else {
                let previous_severity = previous.and_then(|state| state.last_severity);
                let severity_escalated = previous_severity
                    .map(|previous| severity > previous)
                    .unwrap_or(true);
                let last_notification = previous.and_then(|state| state.last_notification_at);
                let cooldown_expired = last_notification
                    .map(|last| now.signed_duration_since(last) >= cooldown)
                    .unwrap_or(true);
                if severity_escalated {
                    events.push(AlertEvent {
                        mount_key: key.clone(),
                        mount_point: disk.mount_point.clone(),
                        display_name: disk.display_name.clone(),
                        used_percent: disk.used_percent,
                        severity: Some(severity),
                        kind: AlertEventKind::Escalation,
                    });
                    if let Some(state) = next_state.alerts.get_mut(&key) {
                        state.last_notification_at = Some(now);
                        state.last_seen_percent = disk.used_percent;
                        state.last_severity = Some(severity);
                    }
                    state_changed = true;
                } else if cooldown_expired {
                    events.push(AlertEvent {
                        mount_key: key.clone(),
                        mount_point: disk.mount_point.clone(),
                        display_name: disk.display_name.clone(),
                        used_percent: disk.used_percent,
                        severity: Some(severity),
                        kind: AlertEventKind::Reminder,
                    });
                    if let Some(state) = next_state.alerts.get_mut(&key) {
                        state.last_notification_at = Some(now);
                        state.last_seen_percent = disk.used_percent;
                        state.last_severity = Some(severity);
                    }
                    state_changed = true;
                } else if let Some(state) = next_state.alerts.get_mut(&key) {
                    if (state.last_seen_percent - disk.used_percent).abs() > f64::EPSILON
                        || state.last_severity != Some(severity)
                    {
                        state.last_seen_percent = disk.used_percent;
                        state.last_severity = Some(severity);
                        state_changed = true;
                    }
                }
            }
        } else if was_active && disk.used_percent <= recovery_threshold {
            if should_send_recovery {
                events.push(AlertEvent {
                    mount_key: key.clone(),
                    mount_point: disk.mount_point.clone(),
                    display_name: disk.display_name.clone(),
                    used_percent: disk.used_percent,
                    severity: None,
                    kind: AlertEventKind::Recovery,
                });
            }
            next_state.alerts.remove(&key);
            state_changed = true;
        } else if let Some(state) = next_state.alerts.get_mut(&key) {
            if (state.last_seen_percent - disk.used_percent).abs() > f64::EPSILON {
                state.last_seen_percent = disk.used_percent;
                state_changed = true;
            }
        }
    }

    for (key, state) in next_state.alerts.iter_mut() {
        if !seen_keys.contains(key) && state.active {
            state.last_seen_percent = 0.0;
        }
    }

    NotificationDecision {
        report_kind: choose_report_kind(&events),
        events,
        state: next_state,
        state_changed,
    }
}

fn build_email_body(
    cfg: &config::Config,
    disks: &[DiskInfo],
    system_info: &system::SystemInfo,
    report_kind: ReportKind,
    thresholds: AlertThresholds,
    recovery_threshold: f64,
    events: &[AlertEvent],
    trends: &BTreeMap<String, TrendInfo>,
) -> String {
    let display_name = cfg
        .friendly_name
        .as_deref()
        .unwrap_or(&system_info.hostname);
    let now: DateTime<Local> = Local::now();
    let datetime = now.format("%d-%m-%Y %H:%M:%S").to_string();
    let os_info = format!(
        "{} {} {}",
        system_info.os_name, system_info.os_version, system_info.architecture
    );
    let problem_count = alert_disks(disks, thresholds).len();
    let status_label = match report_kind {
        ReportKind::Forced => "RAPORT",
        ReportKind::Alert => "ALERTA",
        ReportKind::Reminder => "REMINDER",
        ReportKind::Recovery => "REVENIRE",
    };
    let highest_severity = disks
        .iter()
        .filter_map(|disk| severity_for_percent(disk.used_percent, thresholds))
        .max();
    let status_color = match report_kind {
        ReportKind::Alert | ReportKind::Reminder => {
            highest_severity.map(severity_color).unwrap_or("#b42318")
        }
        ReportKind::Forced | ReportKind::Recovery => "#027a48",
    };
    let event_map: BTreeMap<&str, AlertEventKind> = events
        .iter()
        .map(|event| (event.mount_key.as_str(), event.kind))
        .collect();
    let event_summary = if events.is_empty() {
        "Nu exista evenimente noi; raport complet.".to_string()
    } else {
        events
            .iter()
            .map(|event| {
                format!(
                    "{}{}: {} ({:.2}% ocupat)",
                    event_label(event.kind),
                    event
                        .severity
                        .map(|severity| format!(" {}", severity_label(severity)))
                        .unwrap_or_default(),
                    escape_html(&event.display_name),
                    event.used_percent
                )
            })
            .collect::<Vec<_>>()
            .join("<br>")
    };
    let trend_summary = trends
        .iter()
        .filter_map(|(key, trend)| {
            if trend.abnormal_growth {
                disks
                    .iter()
                    .find(|disk| mount_key(disk) == *key)
                    .map(|disk| {
                        format!(
                            "{}: crestere anormala fata de media ultimelor 7 zile",
                            escape_html(&disk.display_name)
                        )
                    })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let trend_summary = if trend_summary.is_empty() {
        "Nu exista cresteri anormale detectate.".to_string()
    } else {
        trend_summary.join("<br>")
    };
    let executive_summary = if report_kind == ReportKind::Forced {
        build_executive_summary(disks, thresholds, trends)
    } else {
        String::new()
    };

    let mut rows = String::new();
    for disk in disks {
        let key = mount_key(disk);
        let event_kind = event_map.get(key.as_str()).copied();
        let trend = trends.get(&key);
        let severity = severity_for_percent(disk.used_percent, thresholds);
        let row_color = if let Some(severity) = severity {
            severity_row_color(severity)
        } else if event_kind == Some(AlertEventKind::Recovery) {
            "#ecfdf3"
        } else {
            "#f0fdf4"
        };
        let badge = if let Some(kind) = event_kind {
            match kind {
                AlertEventKind::Alert => "<span class=\"badge badge-alert\">Nou</span>",
                AlertEventKind::Escalation => "<span class=\"badge badge-alert\">Escaladat</span>",
                AlertEventKind::Reminder => "<span class=\"badge badge-alert\">Reminder</span>",
                AlertEventKind::Recovery => "<span class=\"badge badge-ok\">Revenit</span>",
            }
        } else if let Some(severity) = severity {
            match severity {
                AlertSeverity::Warning => "<span class=\"badge badge-warning\">WARNING</span>",
                AlertSeverity::Critical => "<span class=\"badge badge-alert\">CRITICAL</span>",
                AlertSeverity::Emergency => {
                    "<span class=\"badge badge-emergency\">EMERGENCY</span>"
                }
            }
        } else {
            "<span class=\"badge badge-ok\">OK</span>"
        };
        rows.push_str(&format!(
            "<tr style=\"background:{}\">\
                <td class=\"mount\">{}</td>\
                <td>{}</td>\
                <td class=\"num\">{:.2} GB</td>\
                <td class=\"num\">{:.2} GB</td>\
                <td class=\"num\">{:.2} GB</td>\
                <td class=\"num\">{:.2}%</td>\
                <td class=\"num\">{:.2}%</td>\
                <td class=\"num\">{}</td>\
                <td class=\"num\">{}</td>\
                <td class=\"num\">{}</td>\
                <td>{}</td>\
                <td>{}</td>\
                <td>{}</td>\
             </tr>",
            row_color,
            escape_html(&disk.mount_point),
            escape_html(&disk.file_system),
            bytes_to_gb(disk.total_space),
            bytes_to_gb(disk.used_space),
            bytes_to_gb(disk.available_space),
            disk.free_percent,
            disk.used_percent,
            trend
                .map(|trend| format_growth(trend.growth_24h))
                .unwrap_or_else(|| "N/A".to_string()),
            trend
                .map(|trend| format_growth(trend.growth_7d))
                .unwrap_or_else(|| "N/A".to_string()),
            trend
                .map(|trend| format_growth(trend.growth_30d))
                .unwrap_or_else(|| "N/A".to_string()),
            trend
                .map(|trend| format_time_to_full(trend.time_to_full_seconds))
                .unwrap_or_else(|| "N/A".to_string()),
            severity_html(severity),
            badge
        ));
    }

    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    body {{ margin:0; padding:0; background:#f4f6f8; font-family:Arial, Helvetica, sans-serif; color:#111827; }}
    .wrap {{ max-width:920px; margin:0 auto; padding:24px; }}
    .panel {{ background:#ffffff; border:1px solid #d9e2ec; border-radius:8px; overflow:hidden; }}
    .header {{ padding:22px 24px; background:#0f172a; color:#ffffff; }}
    .header h1 {{ margin:0 0 8px; font-size:22px; font-weight:700; }}
    .header p {{ margin:0; color:#cbd5e1; font-size:14px; }}
    .executive {{ padding:20px 24px; border-bottom:1px solid #e5e7eb; }}
    .executive h2 {{ margin:0 0 14px; font-size:18px; }}
    .executive h3 {{ margin:18px 0 8px; font-size:14px; color:#344054; }}
    .kpis {{ display:grid; grid-template-columns:repeat(5, 1fr); gap:8px; }}
    .kpis div {{ border:1px solid #e5e7eb; border-radius:6px; padding:10px; background:#f8fafc; }}
    .kpis span {{ display:block; color:#667085; font-size:11px; font-weight:700; }}
    .kpis strong {{ display:block; margin-top:4px; font-size:18px; }}
    table.mini {{ width:100%; border-collapse:collapse; font-size:12px; }}
    table.mini th {{ background:#f8fafc; color:#344054; text-align:left; padding:8px; border-bottom:1px solid #e5e7eb; }}
    table.mini td {{ padding:8px; border-bottom:1px solid #edf2f7; }}
    .recommendations {{ margin:0; padding-left:16px; color:#475467; font-size:12px; line-height:1.4; }}
    .recommendations li {{ margin:3px 0; font-weight:400; }}
    .empty {{ margin:0; color:#667085; font-size:12px; }}
    .summary {{ padding:18px 24px; border-bottom:1px solid #e5e7eb; }}
    .status {{ display:inline-block; padding:6px 10px; border-radius:6px; background:{status_color}; color:#ffffff; font-weight:700; font-size:12px; letter-spacing:.3px; }}
    .meta {{ width:100%; margin-top:14px; border-collapse:collapse; font-size:14px; }}
    .meta td {{ padding:4px 0; }}
    .meta td:first-child {{ color:#344054; width:180px; font-weight:700; }}
    table.disks {{ width:100%; border-collapse:collapse; font-size:13px; }}
    table.disks th {{ background:#f8fafc; color:#344054; text-align:left; padding:10px 12px; border-bottom:1px solid #e5e7eb; }}
    table.disks td {{ padding:10px 12px; border-bottom:1px solid #edf2f7; vertical-align:middle; }}
    table.disks td.mount {{ font-weight:700; color:#111827; }}
    .num {{ text-align:right; white-space:nowrap; }}
    .badge {{ display:inline-block; min-width:54px; text-align:center; padding:4px 8px; border-radius:999px; font-size:12px; font-weight:700; }}
    .badge-ok {{ background:#bbf7d0; color:#166534; }}
    .badge-warning {{ background:#fde68a; color:#92400e; }}
    .badge-alert {{ background:#fecaca; color:#991b1b; }}
    .badge-emergency {{ background:#fca5a5; color:#7f1d1d; }}
    .text-ok {{ color:#15803d; font-weight:800; }}
    .text-warning {{ color:#a16207; font-weight:800; }}
    .text-critical {{ color:#b91c1c; font-weight:800; }}
    .text-emergency {{ color:#7f1d1d; font-weight:900; }}
    .footer {{ padding:14px 24px; color:#667085; font-size:12px; }}
  </style>
</head>
<body>
  <div class="wrap">
    <div class="panel">
      <div class="header">
        <h1>Raport spatiu de stocare</h1>
        <p>{display_name} ({hostname})</p>
      </div>
      {executive_summary}
      <div class="summary">
        <span class="status">{status_label}</span>
        <table class="meta">
          <tr><td>Sistem</td><td>{os_info}</td></tr>
          <tr><td>Hostname</td><td>{hostname}</td></tr>
          <tr><td>Data raport</td><td>{datetime}</td></tr>
          <tr><td>Mod rulare</td><td>{mode}</td></tr>
          <tr><td>Praguri severitate</td><td>Warning {warning_threshold:.2}% / Critical {critical_threshold:.2}% / Emergency {emergency_threshold:.2}% ocupat</td></tr>
          <tr><td>Prag revenire</td><td>{recovery_threshold:.2}% ocupat</td></tr>
          <tr><td>Mount point-uri monitorizate</td><td>{disk_count}</td></tr>
          <tr><td>Mount point-uri peste prag</td><td>{problem_count}</td></tr>
          <tr><td>Evenimente</td><td>{event_summary}</td></tr>
          <tr><td>Trend</td><td>{trend_summary}</td></tr>
        </table>
      </div>
      <table class="disks">
        <thead>
          <tr>
            <th>Punct montare</th>
            <th>Sistem fisiere</th>
            <th class="num">Spatiu total</th>
            <th class="num">Spatiu utilizat</th>
            <th class="num">Spatiu disponibil</th>
            <th class="num">Spatiu liber</th>
            <th class="num">Grad ocupare</th>
            <th class="num">Crestere 24h</th>
            <th class="num">Crestere 7 zile</th>
            <th class="num">Crestere 30 zile</th>
            <th>Estimare umplere</th>
            <th>Severitate</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>{rows}</tbody>
      </table>
      <div class="footer">Raport generat automat de diskmon-mail.</div>
    </div>
  </div>
</body>
</html>"#,
        display_name = escape_html(display_name),
        hostname = escape_html(&system_info.hostname),
        os_info = escape_html(&os_info),
        datetime = escape_html(&datetime),
        mode = match report_kind {
            ReportKind::Forced => "Raport zilnic / la cerere",
            ReportKind::Alert | ReportKind::Reminder | ReportKind::Recovery => {
                "Verificare periodica"
            }
        },
        warning_threshold = thresholds.warning,
        critical_threshold = thresholds.critical,
        emergency_threshold = thresholds.emergency,
        recovery_threshold = recovery_threshold,
        disk_count = disks.len(),
        problem_count = problem_count,
        event_summary = event_summary,
        trend_summary = trend_summary,
        executive_summary = executive_summary,
        status_label = status_label,
        status_color = status_color,
        rows = rows
    )
}

async fn send_system_report(
    cfg: &config::Config,
    disks: &[DiskInfo],
    system_info: &system::SystemInfo,
    report_kind: ReportKind,
    events: &[AlertEvent],
    trends: &BTreeMap<String, TrendInfo>,
    debug_enabled: bool,
) -> Result<(), String> {
    if !cfg.mail_enabled {
        println!(
            "{} Raport generat pentru {} mount point-uri. Email-ul nu este trimis.",
            "[TEST MODE]".yellow().bold(),
            disks.len().to_string().cyan()
        );
        return Ok(());
    }

    let display_name = cfg
        .friendly_name
        .as_deref()
        .unwrap_or(&system_info.hostname);
    let os_info = format!(
        "{} {} {}",
        system_info.os_name, system_info.os_version, system_info.architecture
    );
    let thresholds = alert_thresholds(cfg);
    let recovery_threshold = recovery_threshold(cfg, thresholds.warning);
    let prefix = subject_prefix(report_kind, disks, thresholds);
    let subject = match report_kind {
        ReportKind::Forced => format!(
            "{} [RAPORT ZILNIC] System Disk Report - {} ({})",
            prefix, display_name, os_info
        ),
        ReportKind::Alert => format!(
            "{} [DEPĂSIRE PRAG DE STOCARE] System Disk Report - {} ({})",
            prefix, display_name, os_info
        ),
        ReportKind::Reminder => format!(
            "{} [STOCARE] System Disk Report - {} ({})",
            prefix, display_name, os_info
        ),
        ReportKind::Recovery => format!(
            "{} [STOCARE] System Disk Report - {} ({})",
            prefix, display_name, os_info
        ),
    };
    let body = build_email_body(
        cfg,
        disks,
        system_info,
        report_kind,
        thresholds,
        recovery_threshold,
        events,
        trends,
    );

    let mut builder = Message::builder().from(
        cfg.email_from
            .parse()
            .map_err(|e| format!("Adresa expeditor invalida: {e}"))?,
    );
    for addr in cfg.email_to.split(',') {
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        builder = builder.to(addr
            .parse()
            .map_err(|e| format!("Adresa destinatar invalida '{}': {}", addr, e))?);
    }

    let email = builder
        .subject(subject)
        .header(ContentType::TEXT_HTML)
        .body(body)
        .map_err(|e| format!("Nu s-a putut construi email-ul: {e}"))?;

    let use_auth = !(cfg.smtp_user.trim().is_empty() && cfg.smtp_pass.trim().is_empty());
    let security = cfg
        .smtp_security
        .as_deref()
        .unwrap_or("starttls")
        .to_lowercase();
    if debug_enabled {
        debug!("smtp_security: {:?}", cfg.smtp_security);
    }

    let mailer = match security.as_str() {
        "none" => {
            let mut builder =
                SmtpTransport::builder_dangerous(&cfg.smtp_server).port(cfg.smtp_port);
            if use_auth {
                builder = builder.credentials(Credentials::new(
                    cfg.smtp_user.clone(),
                    cfg.smtp_pass.clone(),
                ));
            }
            builder.build()
        }
        "ssl" => {
            let tls = TlsParameters::new(cfg.smtp_server.clone())
                .map_err(|e| format!("Eroare parametri TLS: {e}"))?;
            let mut builder = SmtpTransport::relay(&cfg.smtp_server)
                .map_err(|e| format!("Eroare SMTP relay: {e}"))?
                .port(cfg.smtp_port)
                .tls(Tls::Wrapper(tls));
            if use_auth {
                builder = builder.credentials(Credentials::new(
                    cfg.smtp_user.clone(),
                    cfg.smtp_pass.clone(),
                ));
            }
            builder.build()
        }
        _ => {
            let mut builder = SmtpTransport::relay(&cfg.smtp_server)
                .map_err(|e| format!("Eroare SMTP relay: {e}"))?
                .port(cfg.smtp_port);
            if use_auth {
                builder = builder.credentials(Credentials::new(
                    cfg.smtp_user.clone(),
                    cfg.smtp_pass.clone(),
                ));
            }
            builder.build()
        }
    };

    let mut backoff = ExponentialBackoff::default();
    backoff.max_elapsed_time = Some(Duration::from_secs(300));
    backoff.initial_interval = Duration::from_secs(1);
    backoff.max_interval = Duration::from_secs(30);

    let mut attempt = 1;
    let max_attempts = 3;
    loop {
        match mailer.send(&email) {
            Ok(_) => break,
            Err(e) => {
                error!("Incercarea SMTP {} a esuat: {}", attempt, e);
                if attempt >= max_attempts {
                    return Err(format!(
                        "Eroare SMTP dupa {} incercari: {}",
                        max_attempts, e
                    ));
                }
                if let Some(delay) = backoff.next_backoff() {
                    warn!("Reincerc SMTP in {:?}...", delay);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                } else {
                    return Err(format!("Eroare SMTP: {}", e));
                }
            }
        }
    }

    println!(
        "{} Raport trimis pentru {} mount point-uri{}.",
        "SUCCESS".green().bold(),
        disks.len().to_string().cyan(),
        if report_kind == ReportKind::Forced {
            " (fortat)".yellow()
        } else {
            "".normal()
        }
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    init_colors();

    let cfg = match config::load_config(config::config_path()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{} {}", "Eroare configurare:".red().bold(), e);
            std::process::exit(2);
        }
    };

    let debug_enabled = cfg.debug.unwrap_or(false);
    env_logger::Builder::from_default_env()
        .filter_level(if debug_enabled {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .init();

    let cli = Cli::parse();
    let thresholds = alert_thresholds(&cfg);
    let system_info = system::get_system_info();
    let now = Utc::now();

    println!(
        "{} {} {} {} ({})",
        "Sistem:".blue().bold(),
        system_info.os_name.green(),
        system_info.os_version.green(),
        system_info.architecture.green(),
        system_info.hostname.cyan()
    );
    println!(
        "{}",
        "Colectez informatiile despre spatiul de stocare...".yellow()
    );

    let disks = get_monitored_disks(&cfg, debug_enabled);
    if disks.is_empty() {
        eprintln!(
            "{}",
            "Nu a fost gasit niciun mount point monitorizabil."
                .red()
                .bold()
        );
        std::process::exit(1);
    }

    let problem_disks = alert_disks(&disks, thresholds);
    let current_state = load_alert_state(debug_enabled);
    let trends = calculate_trends(&current_state, &disks, now);
    println!(
        "{} {} mount point-uri. Praguri: WARNING {:.2}% / CRITICAL {:.2}% / EMERGENCY {:.2}% ocupat.",
        "Monitorizare:".blue().bold(),
        disks.len().to_string().green(),
        thresholds.warning,
        thresholds.critical,
        thresholds.emergency
    );

    for disk in &disks {
        let severity = severity_for_percent(disk.used_percent, thresholds);
        let marker = match severity {
            Some(AlertSeverity::Warning) => "!".yellow().bold(),
            Some(AlertSeverity::Critical) | Some(AlertSeverity::Emergency) => "!".red().bold(),
            None => "OK".green().bold(),
        };
        let used_percent = match severity {
            Some(AlertSeverity::Warning) => format!("{:.2}", disk.used_percent).yellow().bold(),
            Some(AlertSeverity::Critical) | Some(AlertSeverity::Emergency) => {
                format!("{:.2}", disk.used_percent).red().bold()
            }
            None => format!("{:.2}", disk.used_percent).green().bold(),
        };
        let severity_text = severity.map(severity_label).unwrap_or("OK");
        println!(
            "  {} {}: {}% ocupat, severitate {}, {:.2}% liber ({:.2} GB disponibil din {:.2} GB)",
            marker,
            disk.display_name.cyan(),
            used_percent,
            severity_text,
            disk.free_percent,
            bytes_to_gb(disk.available_space),
            bytes_to_gb(disk.total_space)
        );
    }

    if cli.json {
        #[derive(serde::Serialize)]
        struct JsonOutput {
            system_info: system::SystemInfo,
            disks: Vec<DiskInfo>,
            thresholds: AlertThresholds,
            threshold_meaning: &'static str,
            trends: BTreeMap<String, TrendInfo>,
            alerts: Vec<String>,
        }

        let alerts = problem_disks
            .iter()
            .map(|disk| {
                format!(
                    "{}: {:.2}% ocupat ({})",
                    disk.display_name,
                    disk.used_percent,
                    severity_for_percent(disk.used_percent, thresholds)
                        .map(severity_label)
                        .unwrap_or("OK")
                )
            })
            .collect();
        let output = JsonOutput {
            system_info,
            disks,
            thresholds,
            threshold_meaning: "procent ocupat",
            trends,
            alerts,
        };
        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                error!("Nu s-a putut serializa JSON-ul: {}", e);
                std::process::exit(1);
            }
        }
        let mut next_state = current_state;
        update_history(&mut next_state, &output.disks, now);
        if let Err(e) = save_alert_state(&next_state) {
            eprintln!("{} {}", "ERROR".red().bold(), e);
            std::process::exit(2);
        }
        return;
    }

    let mut errors_occurred = false;
    if cli.force_mail {
        println!(
            "\n{}",
            "Mod raport complet: trimit raportul pentru toate mount point-urile..."
                .yellow()
                .bold()
        );
        if let Err(e) = send_system_report(
            &cfg,
            &disks,
            &system_info,
            ReportKind::Forced,
            &[],
            &trends,
            debug_enabled,
        )
        .await
        {
            eprintln!(
                "{} {}",
                "ERROR Nu s-a putut trimite raportul:".red().bold(),
                e
            );
            errors_occurred = true;
        } else {
            let mut next_state = current_state;
            update_history(&mut next_state, &disks, now);
            if let Err(e) = save_alert_state(&next_state) {
                eprintln!("{} {}", "ERROR".red().bold(), e);
                errors_occurred = true;
            }
        }
    } else {
        let mut decision = evaluate_alert_state(&cfg, &disks, &current_state, thresholds, now);
        update_history(&mut decision.state, &disks, now);
        decision.state_changed = true;

        if !problem_disks.is_empty() {
            println!(
                "\n{} {} mount point-uri peste prag:",
                "Alarma activa pentru".red().bold(),
                problem_disks.len().to_string().red().bold()
            );
        }
        for disk in &problem_disks {
            let severity = severity_for_percent(disk.used_percent, thresholds)
                .map(severity_label)
                .unwrap_or("OK");
            println!(
                "  {} {}: {:.2}% ocupat ({})",
                "!".red().bold(),
                disk.display_name.cyan(),
                disk.used_percent,
                severity
            );
        }

        if let Some(report_kind) = decision.report_kind {
            println!(
                "\n{} Evenimente pentru email:",
                "Notificare".yellow().bold()
            );
            for event in &decision.events {
                println!(
                    "  {} {}: {:.2}% ocupat{}",
                    event_label(event.kind),
                    event.display_name.cyan(),
                    event.used_percent,
                    event
                        .severity
                        .map(|severity| format!(" ({})", severity_label(severity)))
                        .unwrap_or_default()
                );
            }
            if let Err(e) = send_system_report(
                &cfg,
                &disks,
                &system_info,
                report_kind,
                &decision.events,
                &trends,
                debug_enabled,
            )
            .await
            {
                eprintln!(
                    "{} {}",
                    "ERROR Nu s-a putut trimite notificarea:".red().bold(),
                    e
                );
                errors_occurred = true;
            } else if let Err(e) = save_alert_state(&decision.state) {
                eprintln!("{} {}", "ERROR".red().bold(), e);
                errors_occurred = true;
            }
        } else {
            if problem_disks.is_empty() {
                println!(
                    "\n{} Toate mount point-urile sunt sub pragul de {:.2}% ocupare.",
                    "OK".green().bold(),
                    thresholds.warning
                );
            } else {
                println!(
                    "\n{} Alerta este deja activa; nu trimit email pana expira cooldown-ul de {} ore.",
                    "INFO".yellow().bold(),
                    cfg.alert_cooldown_hours.unwrap_or(12)
                );
            }

            if decision.state_changed {
                if let Err(e) = save_alert_state(&decision.state) {
                    eprintln!("{} {}", "ERROR".red().bold(), e);
                    errors_occurred = true;
                }
            }
        }
    }

    if errors_occurred {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> config::Config {
        config::Config {
            mail_enabled: true,
            smtp_server: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_user: "user@example.com".to_string(),
            smtp_pass: "password".to_string(),
            email_from: "admin@example.com".to_string(),
            email_to: "alerts@example.com".to_string(),
            smtp_security: Some("starttls".to_string()),
            threshold_percent: Some(85.0),
            warning_threshold_percent: Some(85.0),
            critical_threshold_percent: Some(90.0),
            emergency_threshold_percent: Some(95.0),
            alert_cooldown_hours: Some(12),
            recovery_threshold_percent: Some(82.0),
            send_recovery_email: Some(true),
            debug: Some(false),
            friendly_name: Some("test".to_string()),
            excluded_disks: Some(vec![]),
        }
    }

    fn disk(used_percent: f64) -> DiskInfo {
        DiskInfo {
            mount_point: "/var".to_string(),
            display_name: "/var (/dev/sda2)".to_string(),
            file_system: "ext4".to_string(),
            total_space: 100,
            used_space: used_percent as u64,
            available_space: 100_u64.saturating_sub(used_percent as u64),
            used_percent,
            free_percent: 100.0 - used_percent,
        }
    }

    fn disk_with_space(used_space: u64, total_space: u64) -> DiskInfo {
        DiskInfo {
            mount_point: "/var".to_string(),
            display_name: "/var (/dev/sda2)".to_string(),
            file_system: "ext4".to_string(),
            total_space,
            used_space,
            available_space: total_space.saturating_sub(used_space),
            used_percent: percent(used_space, total_space),
            free_percent: percent(total_space.saturating_sub(used_space), total_space),
        }
    }

    #[test]
    fn alert_state_sends_new_alert_then_suppresses_until_cooldown() {
        let cfg = test_config();
        let now = Utc::now();
        let state = empty_alert_state();
        let disks = vec![disk(90.0)];
        let thresholds = alert_thresholds(&cfg);

        let first = evaluate_alert_state(&cfg, &disks, &state, thresholds, now);
        assert_eq!(first.report_kind, Some(ReportKind::Alert));
        assert_eq!(first.events[0].kind, AlertEventKind::Alert);
        assert_eq!(first.events[0].severity, Some(AlertSeverity::Critical));

        let suppressed = evaluate_alert_state(
            &cfg,
            &disks,
            &first.state,
            thresholds,
            now + chrono::Duration::hours(6),
        );
        assert_eq!(suppressed.report_kind, None);
        assert!(suppressed.events.is_empty());

        let reminder = evaluate_alert_state(
            &cfg,
            &disks,
            &first.state,
            thresholds,
            now + chrono::Duration::hours(12),
        );
        assert_eq!(reminder.report_kind, Some(ReportKind::Reminder));
        assert_eq!(reminder.events[0].kind, AlertEventKind::Reminder);
    }

    #[test]
    fn alert_state_waits_for_recovery_threshold_before_recovery_email() {
        let cfg = test_config();
        let now = Utc::now();
        let thresholds = alert_thresholds(&cfg);
        let active =
            evaluate_alert_state(&cfg, &[disk(90.0)], &empty_alert_state(), thresholds, now);

        let still_active = evaluate_alert_state(
            &cfg,
            &[disk(83.0)],
            &active.state,
            thresholds,
            now + chrono::Duration::hours(1),
        );
        assert_eq!(still_active.report_kind, None);
        assert!(still_active.state.alerts.values().any(|state| state.active));

        let recovered = evaluate_alert_state(
            &cfg,
            &[disk(82.0)],
            &active.state,
            thresholds,
            now + chrono::Duration::hours(2),
        );
        assert_eq!(recovered.report_kind, Some(ReportKind::Recovery));
        assert_eq!(recovered.events[0].kind, AlertEventKind::Recovery);
        assert!(recovered.state.alerts.is_empty());
    }

    #[test]
    fn alert_state_sends_immediately_when_severity_escalates() {
        let cfg = test_config();
        let now = Utc::now();
        let thresholds = alert_thresholds(&cfg);
        let warning =
            evaluate_alert_state(&cfg, &[disk(86.0)], &empty_alert_state(), thresholds, now);

        let escalated = evaluate_alert_state(
            &cfg,
            &[disk(96.0)],
            &warning.state,
            thresholds,
            now + chrono::Duration::hours(1),
        );
        assert_eq!(escalated.report_kind, Some(ReportKind::Alert));
        assert_eq!(escalated.events[0].kind, AlertEventKind::Escalation);
        assert_eq!(escalated.events[0].severity, Some(AlertSeverity::Emergency));
    }

    #[test]
    fn trends_calculate_growth_and_time_to_full() {
        let now = Utc::now();
        let gib = 1024_u64 * 1024 * 1024;
        let disk = disk_with_space(64 * gib, 100 * gib);
        let mut state = empty_alert_state();
        state.history.insert(
            mount_key(&disk),
            vec![
                DiskHistorySample {
                    timestamp: now - chrono::Duration::hours(24),
                    total_space: 100 * gib,
                    used_space: 52 * gib,
                    available_space: 48 * gib,
                    used_percent: 52.0,
                },
                DiskHistorySample {
                    timestamp: now - chrono::Duration::days(7),
                    total_space: 100 * gib,
                    used_space: 44 * gib,
                    available_space: 56 * gib,
                    used_percent: 44.0,
                },
            ],
        );

        let trends = calculate_trends(&state, &[disk.clone()], now);
        let trend = trends.get(&mount_key(&disk)).unwrap();

        assert_eq!(trend.growth_24h, Some((12 * gib) as i64));
        assert_eq!(trend.growth_7d, Some((20 * gib) as i64));
        assert_eq!(trend.time_to_full_seconds, Some(3 * 86_400));
        assert!(trend.abnormal_growth);
    }

    #[test]
    fn executive_summary_contains_daily_sections() {
        let cfg = test_config();
        let thresholds = alert_thresholds(&cfg);
        let disk = disk(96.0);
        let mut trends = BTreeMap::new();
        trends.insert(
            mount_key(&disk),
            TrendInfo {
                growth_24h: Some(12 * 1024 * 1024 * 1024),
                growth_7d: Some(20 * 1024 * 1024 * 1024),
                growth_30d: None,
                time_to_full_seconds: Some(3 * 86_400),
                abnormal_growth: true,
            },
        );

        let html = build_executive_summary(&[disk], thresholds, &trends);

        assert!(html.contains("Sumar executiv"));
        assert!(html.contains("Top 5 cele mai ocupate"));
        assert!(html.contains("Top 5 cele mai mari cresteri"));
        assert!(html.contains("Estimari time-to-full"));
        assert!(html.contains("Recomandari de actiune"));
        assert!(html.contains("EMERGENCY"));
    }

    #[test]
    fn subject_prefix_uses_unicode_status_markers() {
        let cfg = test_config();
        let thresholds = alert_thresholds(&cfg);
        let ok_disk = disk(50.0);
        let alert_disk = disk(95.0);

        assert_eq!(
            subject_prefix(ReportKind::Forced, &[ok_disk], thresholds),
            "🟢 [RAPORT OK]"
        );
        assert_eq!(
            subject_prefix(ReportKind::Forced, &[alert_disk], thresholds),
            "🔴 [RAPORT ALERTĂ]"
        );
        assert_eq!(
            subject_prefix(ReportKind::Alert, &[], thresholds),
            "🔴 [ALARMĂ]"
        );
        assert_eq!(
            subject_prefix(ReportKind::Recovery, &[], thresholds),
            "🟢 [REVENIRE OK]"
        );
    }
}
