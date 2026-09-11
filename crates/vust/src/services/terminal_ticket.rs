//! 宿主机终端一次性票据服务。

use ring::{digest, rand::SecureRandom};
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::time::Instant;
use vust_contracts::terminal::TerminalTicketContext;

const DEFAULT_TICKET_TTL: Duration = Duration::from_secs(15);
const DEFAULT_MAX_PENDING_TICKETS: usize = 1024;

struct PendingTicket {
    context: TerminalTicketContext,
    expires_at: Instant,
}

/// 仅在 Master 内存中保存哈希的一次性终端票据仓库。
pub struct TerminalTicketStore {
    pending: Mutex<HashMap<String, PendingTicket>>,
    max_pending: usize,
}

impl Default for TerminalTicketStore {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PENDING_TICKETS)
    }
}

impl TerminalTicketStore {
    /// 创建具有固定容量上限的票据仓库。
    pub fn new(max_pending: usize) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            max_pending,
        }
    }

    /// 签发一张 15 秒内有效的一次性终端票据。
    pub fn issue(&self, context: TerminalTicketContext) -> anyhow::Result<String> {
        self.issue_with_ttl(context, DEFAULT_TICKET_TTL)
    }

    /// 原子消费票据；票据无论节点是否匹配都不能再次使用。
    pub fn consume(&self, ticket: &str, node_id: &str) -> Option<TerminalTicketContext> {
        let mut pending = self.pending.lock().ok()?;
        let entry = pending.remove(&ticket_hash(ticket))?;
        if entry.expires_at <= Instant::now() || entry.context.node_id != node_id {
            return None;
        }
        Some(entry.context)
    }

    /// 主动撤销尚未被消费的票据。
    pub fn revoke(&self, ticket: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&ticket_hash(ticket));
        }
    }

    fn issue_with_ttl(
        &self,
        context: TerminalTicketContext,
        ttl: Duration,
    ) -> anyhow::Result<String> {
        let mut bytes = [0_u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| anyhow::anyhow!("failed to generate terminal ticket"))?;
        let ticket = hex::encode(bytes);
        let now = Instant::now();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal ticket store is unavailable"))?;
        pending.retain(|_, entry| entry.expires_at > now);
        if pending.len() >= self.max_pending {
            anyhow::bail!("terminal ticket capacity has been reached");
        }
        pending.insert(
            ticket_hash(&ticket),
            PendingTicket {
                context,
                expires_at: now + ttl,
            },
        );
        Ok(ticket)
    }
}

fn ticket_hash(ticket: &str) -> String {
    hex::encode(digest::digest(&digest::SHA256, ticket.as_bytes()).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(node_id: &str) -> TerminalTicketContext {
        TerminalTicketContext {
            actor_name: "admin".to_string(),
            client_ip: "192.0.2.10".to_string(),
            trace_id: "trace-1".to_string(),
            node_id: node_id.to_string(),
        }
    }

    #[test]
    fn ticket_is_single_use_and_bound_to_node() {
        let store = TerminalTicketStore::default();
        let ticket = store.issue(context("node-1")).unwrap();
        assert!(store.consume(&ticket, "node-2").is_none());
        assert!(store.consume(&ticket, "node-1").is_none());

        let ticket = store.issue(context("node-1")).unwrap();
        assert_eq!(
            store.consume(&ticket, "node-1").unwrap().actor_name,
            "admin"
        );
        assert!(store.consume(&ticket, "node-1").is_none());
    }

    #[test]
    fn expired_and_revoked_tickets_are_rejected() {
        let store = TerminalTicketStore::default();
        let expired = store
            .issue_with_ttl(context("local"), Duration::ZERO)
            .unwrap();
        assert!(store.consume(&expired, "local").is_none());

        let revoked = store.issue(context("local")).unwrap();
        store.revoke(&revoked);
        assert!(store.consume(&revoked, "local").is_none());
    }

    #[test]
    fn capacity_is_bounded() {
        let store = TerminalTicketStore::new(1);
        let _ticket = store.issue(context("local")).unwrap();
        assert!(store.issue(context("local")).is_err());
    }
}
