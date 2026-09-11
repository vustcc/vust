//! 节点身份服务：为后续证书绑定与轮换改造预留入口。

use crate::state::DbPool;

/// 占位：后续在 M3 中接入真实身份绑定逻辑。
pub async fn ensure_identity_foundation(_pool: &DbPool, _node_id: &str) -> sqlx::Result<()> {
    Ok(())
}
