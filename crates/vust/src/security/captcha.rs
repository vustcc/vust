//! 验证码服务：生成 4 位数字图片验证码并一次性校验。

use captcha_rs::CaptchaBuilder;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

/// 验证码响应载荷。
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CaptchaPayload {
    pub captcha_id: String,
    pub image: String,
}

#[derive(Debug, Clone)]
struct CaptchaEntry {
    answer: String,
    created_at: Instant,
}

/// 内存验证码服务。
#[derive(Clone)]
pub struct CaptchaService {
    store: Arc<RwLock<HashMap<String, CaptchaEntry>>>,
    ttl: Duration,
}

impl CaptchaService {
    /// 创建验证码服务。
    pub fn new(ttl: Duration) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// 生成新验证码。
    pub async fn generate(&self) -> CaptchaPayload {
        let captcha = CaptchaBuilder::new()
            .length(4)
            .chars(('0'..='9').collect())
            .width(160)
            .height(60)
            .complexity(3)
            .interference_lines(4)
            .interference_ellipses(2)
            .build();
        let id = Uuid::new_v4().to_string();
        let answer = captcha.text.clone();
        self.store.write().await.insert(
            id.clone(),
            CaptchaEntry {
                answer,
                created_at: Instant::now(),
            },
        );
        CaptchaPayload {
            captcha_id: id,
            image: captcha.to_base64(),
        }
    }

    /// 一次性校验验证码。
    pub async fn verify(&self, id: &str, input: &str) -> bool {
        let mut store = self.store.write().await;
        match store.remove(id) {
            Some(entry) if entry.created_at.elapsed() <= self.ttl => entry.answer == input.trim(),
            _ => false,
        }
    }
}

impl Default for CaptchaService {
    fn default() -> Self {
        Self::new(Duration::from_secs(5 * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::CaptchaService;
    use std::time::Duration;

    #[tokio::test]
    async fn generated_captcha_is_numeric_and_single_use() {
        let service = CaptchaService::new(Duration::from_secs(60));
        let payload = service.generate().await;
        assert!(!payload.captcha_id.is_empty());
        assert!(!payload.image.is_empty());

        let answer = service
            .store
            .read()
            .await
            .get(&payload.captcha_id)
            .expect("captcha should be stored")
            .answer
            .clone();
        assert_eq!(answer.len(), 4);
        assert!(answer.chars().all(|ch| ch.is_ascii_digit()));

        assert!(service.verify(&payload.captcha_id, &answer).await);
        assert!(!service.verify(&payload.captcha_id, &answer).await);
    }

    #[tokio::test]
    async fn rejects_wrong_captcha_input() {
        let service = CaptchaService::new(Duration::from_secs(60));
        let payload = service.generate().await;
        let answer = service
            .store
            .read()
            .await
            .get(&payload.captcha_id)
            .expect("captcha should be stored")
            .answer
            .clone();
        let wrong = if answer == "0000" { "1111" } else { "0000" };
        assert!(!service.verify(&payload.captcha_id, wrong).await);
    }
}
