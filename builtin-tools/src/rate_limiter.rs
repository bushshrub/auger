use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tokio::time::Instant;

pub(crate) struct RateLimiter {
    last_request: Arc<Mutex<Option<Instant>>>,
    min_delay: Duration,
}

impl RateLimiter {
    pub fn new(min_delay_ms: u64) -> Self {
        RateLimiter {
            last_request: Arc::new(Mutex::new(None)),
            min_delay: Duration::from_millis(min_delay_ms),
        }
    }

    pub async fn acquire(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(last_time) = *last {
            let elapsed = last_time.elapsed();
            if elapsed < self.min_delay {
                tokio::time::sleep(self.min_delay - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }
}
