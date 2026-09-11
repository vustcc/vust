//! 节点控制面 API 共享类型：创建与更新载荷。

use serde::{Deserialize, Serialize};

/// 新建节点时的请求载荷。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeCreatePayload {
    #[serde(skip_deserializing)]
    pub agent_id: Option<String>,
    pub name: Option<String>,
    pub group_id: Option<String>,
    pub description: Option<String>,
    pub addr: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub pwd: Option<String>,
    pub private_key: Option<String>,
    pub private_key_passphrase: Option<String>,
    pub auth_mode: Option<String>,
    #[serde(skip_deserializing)]
    pub status: Option<String>,
    #[serde(skip_deserializing)]
    pub version: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(skip_deserializing)]
    pub metadata: Option<serde_json::Value>,
    pub install_dir: Option<String>,
    #[serde(rename = "servicePort", alias = "listenPort")]
    pub service_port: Option<String>,
    #[serde(skip_deserializing)]
    pub sync_enabled: Option<bool>,
}

/// 更新节点信息时的请求载荷。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdatePayload {
    #[serde(skip_deserializing)]
    pub agent_id: Option<String>,
    pub name: Option<String>,
    pub group_id: Option<String>,
    pub description: Option<String>,
    pub addr: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub pwd: Option<String>,
    pub private_key: Option<String>,
    pub private_key_passphrase: Option<String>,
    pub auth_mode: Option<String>,
    #[serde(skip_deserializing)]
    pub status: Option<String>,
    #[serde(skip_deserializing)]
    pub version: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(skip_deserializing)]
    pub metadata: Option<serde_json::Value>,
    pub install_dir: Option<String>,
    #[serde(rename = "servicePort", alias = "listenPort")]
    pub service_port: Option<String>,
    #[serde(skip_deserializing)]
    pub sync_enabled: Option<bool>,
    pub vust_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{NodeCreatePayload, NodeUpdatePayload};

    #[test]
    fn client_cannot_assign_internal_node_fields() {
        let create: NodeCreatePayload = serde_json::from_value(serde_json::json!({
            "name": "worker-01",
            "agentId": "forged-agent",
            "status": "ONLINE",
            "version": "forged-version",
            "metadata": { "vust.owner": "system" },
            "syncEnabled": true
        }))
        .unwrap();
        assert!(create.agent_id.is_none());
        assert!(create.status.is_none());
        assert!(create.version.is_none());
        assert!(create.metadata.is_none());
        assert!(create.sync_enabled.is_none());

        let update: NodeUpdatePayload = serde_json::from_value(serde_json::json!({
            "agentId": "forged-agent",
            "status": "retired",
            "metadata": { "vust.owner": "system" }
        }))
        .unwrap();
        assert!(update.agent_id.is_none());
        assert!(update.status.is_none());
        assert!(update.metadata.is_none());
    }
}
