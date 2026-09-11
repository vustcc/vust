//! 登录失败追踪：按客户端 IP 触发验证码与临时锁定。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 单个 IP 的失败记录。
#[derive(Debug, Clone)]
struct FailureRecord {
    count: u32,
    last_failure: Instant,
}

/// 登录失败追踪器。
#[derive(Clone)]
pub struct LoginTracker {
    failures: Arc<RwLock<HashMap<IpAddr, FailureRecord>>>,
    captcha_threshold: u32,
    lockout_threshold: u32,
    lockout_duration: Duration,
}

impl LoginTracker {
    /// 创建默认追踪策略。
    pub fn new() -> Self {
        Self {
            failures: Arc::new(RwLock::new(HashMap::new())),
            captcha_threshold: 1,
            lockout_threshold: 5,
            lockout_duration: Duration::from_secs(15 * 60),
        }
    }

    /// 当前 IP 是否需要验证码。
    pub async fn needs_captcha(&self, ip: &IpAddr) -> bool {
        self.failures
            .read()
            .await
            .get(ip)
            .is_some_and(|record| record.count >= self.captcha_threshold)
    }

    /// 记录一次登录失败。
    pub async fn record_failure(&self, ip: &IpAddr) -> bool {
        let now = Instant::now();
        let mut failures = self.failures.write().await;
        let record = failures
            .entry(*ip)
            .and_modify(|record| {
                record.count += 1;
                record.last_failure = now;
            })
            .or_insert(FailureRecord {
                count: 1,
                last_failure: now,
            });
        record.count == self.lockout_threshold
    }

    /// 清除当前 IP 的失败记录。
    pub async fn clear_failures(&self, ip: &IpAddr) {
        self.failures.write().await.remove(ip);
    }

    /// 获取锁定剩余秒数。
    pub async fn lockout_remaining_secs(&self, ip: &IpAddr) -> Option<u64> {
        let failures = self.failures.read().await;
        let record = failures.get(ip)?;
        if record.count < self.lockout_threshold {
            return None;
        }
        let elapsed = record.last_failure.elapsed();
        if elapsed >= self.lockout_duration {
            return None;
        }
        Some((self.lockout_duration - elapsed).as_secs())
    }
}

impl Default for LoginTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::LoginTracker;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn requires_captcha_after_first_failure() {
        let tracker = LoginTracker::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert!(!tracker.needs_captcha(&ip).await);
        tracker.record_failure(&ip).await;
        assert!(tracker.needs_captcha(&ip).await);
    }

    #[tokio::test]
    async fn locks_after_threshold_and_clears_failures() {
        let tracker = LoginTracker::new();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8));
        for _ in 0..5 {
            tracker.record_failure(&ip).await;
        }
        assert!(tracker.lockout_remaining_secs(&ip).await.is_some());
        tracker.clear_failures(&ip).await;
        assert!(!tracker.needs_captcha(&ip).await);
        assert!(tracker.lockout_remaining_secs(&ip).await.is_none());
    }
}
