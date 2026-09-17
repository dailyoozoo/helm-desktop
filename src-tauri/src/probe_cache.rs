use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

pub(crate) const PROBE_INVALIDATED: &str = "[probe_invalidated] 探测期间配置已刷新，请重试";

#[derive(Clone, Debug)]
pub(crate) struct ProbeToken {
    valid: Arc<AtomicBool>,
}

impl ProbeToken {
    pub(crate) fn is_current(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct ProbeOutcome<T> {
    result: Result<T, String>,
    completed_at: Instant,
    cacheable: bool,
}

#[derive(Debug)]
struct ProbeEntry<T> {
    valid: Arc<AtomicBool>,
    outcome: OnceCell<ProbeOutcome<T>>,
    created_at: Instant,
}

impl<T> ProbeEntry<T> {
    fn pending() -> Self {
        Self {
            valid: Arc::new(AtomicBool::new(true)),
            outcome: OnceCell::new(),
            created_at: Instant::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AsyncProbeCache<T> {
    entries: Arc<Mutex<HashMap<String, Arc<ProbeEntry<T>>>>>,
    ttl: Duration,
    capacity: usize,
}

impl<T: Clone> AsyncProbeCache<T> {
    pub(crate) fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl,
            capacity: capacity.max(1),
        }
    }

    fn make_room(&self, entries: &mut HashMap<String, Arc<ProbeEntry<T>>>) -> bool {
        entries.retain(|_, entry| {
            entry.outcome.get().is_none_or(|outcome| {
                outcome.cacheable && outcome.completed_at.elapsed() < self.ttl
            })
        });
        if entries.len() < self.capacity {
            return true;
        }
        let oldest = entries
            .iter()
            .filter(|(_, entry)| entry.outcome.get().is_some() || Arc::strong_count(entry) == 1)
            .min_by_key(|(_, entry)| {
                entry
                    .outcome
                    .get()
                    .map_or(entry.created_at, |outcome| outcome.completed_at)
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            entries.remove(&key);
            true
        } else {
            false
        }
    }

    pub(crate) async fn get_or_probe<F, Fut>(&self, key: String, probe: F) -> Result<T, String>
    where
        F: FnOnce(ProbeToken) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        self.get_or_probe_if(key, probe, |_| true).await
    }

    pub(crate) async fn get_or_probe_if<F, Fut, Accept>(
        &self,
        key: String,
        probe: F,
        accept: Accept,
    ) -> Result<T, String>
    where
        F: FnOnce(ProbeToken) -> Fut,
        Fut: Future<Output = Result<T, String>>,
        Accept: Fn(&T) -> bool,
    {
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| "探测缓存锁中毒".to_string())?;
            let expired = entries.get(&key).is_some_and(|entry| {
                entry.outcome.get().is_some_and(|outcome| {
                    !outcome.cacheable
                        || outcome.completed_at.elapsed() >= self.ttl
                        || !outcome.result.as_ref().is_ok_and(&accept)
                })
            });
            if expired {
                entries.remove(&key);
            }
            if let Some(entry) = entries.get(&key) {
                entry.clone()
            } else {
                if !self.make_room(&mut entries) {
                    return Err("[probe_cache_busy] 并发探测已达上限，请稍后重试".to_string());
                }
                let entry = Arc::new(ProbeEntry::pending());
                entries.insert(key.clone(), entry.clone());
                entry
            }
        };
        let outcome = entry
            .outcome
            .get_or_init(|| async {
                let result = probe(ProbeToken {
                    valid: entry.valid.clone(),
                })
                .await;
                let cacheable = result.as_ref().is_ok_and(accept);
                ProbeOutcome {
                    result,
                    completed_at: Instant::now(),
                    cacheable,
                }
            })
            .await;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "探测缓存锁中毒".to_string())?;
        if !entry.valid.load(Ordering::Acquire) {
            return entries
                .get(&key)
                .filter(|current| current.valid.load(Ordering::Acquire))
                .and_then(|current| current.outcome.get())
                .filter(|current| current.cacheable && current.completed_at.elapsed() < self.ttl)
                .map(|current| current.result.clone())
                .unwrap_or_else(|| Err(PROBE_INVALIDATED.to_string()));
        }
        if !outcome.cacheable
            && entries
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
        {
            entries.remove(&key);
        }
        outcome.result.clone()
    }

    pub(crate) fn put(&self, key: String, value: T) -> Result<bool, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "探测缓存锁中毒".to_string())?;
        if !entries.contains_key(&key) && !self.make_room(&mut entries) {
            return Ok(false);
        }
        let entry = ProbeEntry::pending();
        let _ = entry.outcome.set(ProbeOutcome {
            result: Ok(value),
            completed_at: Instant::now(),
            cacheable: true,
        });
        if let Some(previous) = entries.insert(key, Arc::new(entry)) {
            previous.valid.store(false, Ordering::Release);
        }
        Ok(true)
    }

    pub(crate) fn invalidate_where(&self, matches: impl Fn(&str) -> bool) -> Result<(), String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "探测缓存锁中毒".to_string())?;
        entries.retain(|key, entry| {
            if matches(key) {
                entry.valid.store(false, Ordering::Release);
                false
            } else {
                true
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn concurrent_callers_share_one_probe_and_success() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 4);
        let calls = AtomicUsize::new(0);
        let probe = |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(7)
        };
        let (first, second, third) = tokio::join!(
            cache.get_or_probe("same".into(), probe),
            cache.get_or_probe("same".into(), probe),
            cache.get_or_probe("same".into(), probe),
        );
        assert_eq!((first.unwrap(), second.unwrap(), third.unwrap()), (7, 7, 7));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            cache
                .get_or_probe("same".into(), |_| async { Ok(9) })
                .await
                .unwrap(),
            7
        );
    }

    #[tokio::test]
    async fn failures_are_shared_in_flight_but_not_cached() {
        let cache = AsyncProbeCache::<usize>::new(Duration::from_secs(60), 4);
        let calls = AtomicUsize::new(0);
        let probe = |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Err("unavailable".into())
        };
        let (first, second) = tokio::join!(
            cache.get_or_probe("same".into(), probe),
            cache.get_or_probe("same".into(), probe),
        );
        assert_eq!(first.unwrap_err(), "unavailable");
        assert_eq!(second.unwrap_err(), "unavailable");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            cache
                .get_or_probe("same".into(), |_| async { Ok(3) })
                .await
                .unwrap(),
            3
        );
    }

    #[tokio::test]
    async fn unverified_results_are_returned_without_caching() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 4);
        assert!(!cache
            .get_or_probe_if("same".into(), |_| async { Ok(false) }, |value| *value)
            .await
            .unwrap());
        assert!(cache
            .get_or_probe("same".into(), |_| async { Ok(true) })
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn successful_results_expire() {
        let cache = AsyncProbeCache::new(Duration::from_millis(5), 4);
        cache.put("same".into(), 1).unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert_eq!(
            cache
                .get_or_probe("same".into(), |_| async { Ok(2) })
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn invalidation_rejects_a_late_probe() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 4);
        let started = Notify::new();
        let release = Notify::new();
        let (result, ()) = tokio::join!(
            cache.get_or_probe("codex:key".into(), |token| {
                let started = &started;
                let release = &release;
                async move {
                    started.notify_one();
                    release.notified().await;
                    assert!(!token.is_current());
                    Ok(1)
                }
            }),
            async {
                started.notified().await;
                cache
                    .invalidate_where(|key| key.starts_with("codex:"))
                    .unwrap();
                release.notify_one();
            }
        );
        assert_eq!(result.unwrap_err(), PROBE_INVALIDATED);
        assert_eq!(
            cache
                .get_or_probe("codex:key".into(), |_| async { Ok(2) })
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn newer_observation_wins_over_an_older_probe() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 4);
        let started = Notify::new();
        let release = Notify::new();
        let (result, ()) = tokio::join!(
            cache.get_or_probe("same".into(), |_| async {
                started.notify_one();
                release.notified().await;
                Ok(1)
            }),
            async {
                started.notified().await;
                cache.put("same".into(), 2).unwrap();
                release.notify_one();
            }
        );
        assert_eq!(result.unwrap(), 2);
    }

    #[tokio::test]
    async fn capacity_is_bounded_without_evicting_active_probes() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 1);
        let started = Notify::new();
        let release = Notify::new();
        let (result, ()) = tokio::join!(
            cache.get_or_probe("first".into(), |_| async {
                started.notify_one();
                release.notified().await;
                Ok(1)
            }),
            async {
                started.notified().await;
                let error = cache
                    .get_or_probe("second".into(), |_| async { Ok(2) })
                    .await
                    .unwrap_err();
                assert!(error.starts_with("[probe_cache_busy]"));
                release.notify_one();
            }
        );
        assert_eq!(result.unwrap(), 1);
        assert_eq!(
            cache
                .get_or_probe("second".into(), |_| async { Ok(2) })
                .await
                .unwrap(),
            2
        );
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancelled_probe_can_be_retried() {
        let cache = AsyncProbeCache::new(Duration::from_secs(60), 1);
        let result = tokio::time::timeout(
            Duration::from_millis(5),
            cache.get_or_probe("same".into(), |_| async {
                std::future::pending::<()>().await;
                Ok(1)
            }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            cache
                .get_or_probe("same".into(), |_| async { Ok(2) })
                .await
                .unwrap(),
            2
        );
    }
}
