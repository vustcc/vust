//! 节点健康摘要采样；系统监控页面使用独立的缓存采样服务。

use chrono::Utc;
use sysinfo::{Networks, System};
use vust_contracts::types::HostSystemSummary;

/// 采集节点健康检查所需的兼容摘要。
pub fn collect_snapshot() -> anyhow::Result<HostSystemSummary> {
    let mut system = System::new_all();
    system.refresh_cpu_usage();
    std::thread::sleep(std::time::Duration::from_millis(120));
    system.refresh_cpu_usage();
    system.refresh_memory();

    let load_average = System::load_average();
    let memory_total_bytes = system.total_memory();
    let memory_used_bytes = memory_total_bytes.saturating_sub(system.available_memory());
    let memory_percent = if memory_total_bytes > 0 {
        memory_used_bytes as f64 / memory_total_bytes as f64 * 100.0
    } else {
        0.0
    };
    let (disk_read_bytes, disk_write_bytes) = collect_disk_io_totals();
    let networks = Networks::new_with_refreshed_list();
    let network_rx_bytes = networks.values().map(|data| data.total_received()).sum();
    let network_tx_bytes = networks.values().map(|data| data.total_transmitted()).sum();

    Ok(HostSystemSummary {
        version: env!("CARGO_PKG_VERSION").to_string(),
        cpu_percent: system.global_cpu_info().cpu_usage(),
        memory_used_bytes,
        memory_total_bytes,
        memory_percent,
        load_avg_1: load_average.one,
        load_avg_5: load_average.five,
        load_avg_15: load_average.fifteen,
        disk_read_bytes,
        disk_write_bytes,
        network_rx_bytes,
        network_tx_bytes,
        collected_at: Utc::now().timestamp(),
    })
}

fn collect_disk_io_totals() -> (u64, u64) {
    let Ok(content) = std::fs::read_to_string("/proc/diskstats") else {
        return (0, 0);
    };
    content
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() < 14 {
                return None;
            }
            let device = std::path::Path::new("/sys/class/block").join(columns[2]);
            if !device.join("device").exists() || device.join("partition").exists() {
                return None;
            }
            Some((
                columns[5].parse::<u64>().ok()?.saturating_mul(512),
                columns[9].parse::<u64>().ok()?.saturating_mul(512),
            ))
        })
        .fold((0_u64, 0_u64), |(reads, writes), (read, write)| {
            (reads.saturating_add(read), writes.saturating_add(write))
        })
}
