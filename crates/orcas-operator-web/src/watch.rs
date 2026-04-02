use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub async fn run_change_watch_loop<Checkpoint, Change, Wait, Fut, OnChange>(
    alive: Arc<AtomicBool>,
    mut checkpoint: Checkpoint,
    mut wait_for_advance: Wait,
    mut on_change: OnChange,
) -> Result<(), String>
where
    Checkpoint: Copy + Send + 'static,
    Wait: FnMut(Checkpoint, Option<u64>) -> Fut,
    Fut: Future<Output = Result<Option<(Checkpoint, Change)>, String>>,
    OnChange: FnMut(Change) -> bool,
{
    while alive.load(Ordering::Acquire) {
        let next = wait_for_advance(checkpoint, Some(30_000)).await?;
        if !alive.load(Ordering::Acquire) {
            break;
        }
        if let Some((next_checkpoint, change)) = next {
            checkpoint = next_checkpoint;
            if !on_change(change) {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_change_watch_loop;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn advances_once_and_stops_when_signaled() {
        let alive = Arc::new(AtomicBool::new(true));
        let advances = Arc::new(AtomicUsize::new(0));
        let wait_calls = Arc::new(Mutex::new(Vec::<u64>::new()));
        let stop_after_first = alive.clone();
        let advances_count = advances.clone();
        let wait_calls_record = wait_calls.clone();
        let result = run_change_watch_loop(
            alive,
            7u64,
            move |checkpoint, _timeout_ms| {
                let wait_calls_record = wait_calls_record.clone();
                async move {
                    wait_calls_record.lock().await.push(checkpoint);
                    if checkpoint == 7 {
                        Ok(Some((8, ())))
                    } else {
                        Ok(None)
                    }
                }
            },
            move |_: ()| {
                advances_count.fetch_add(1, Ordering::SeqCst);
                stop_after_first.store(false, Ordering::Release);
                false
            },
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(advances.load(Ordering::SeqCst), 1);
        let calls: tokio::sync::MutexGuard<'_, Vec<u64>> = wait_calls.lock().await;
        assert_eq!(calls.as_slice(), &[7]);
    }

    #[tokio::test]
    async fn no_change_does_not_trigger_callbacks() {
        let alive = Arc::new(AtomicBool::new(true));
        let advances = Arc::new(AtomicUsize::new(0));
        let stop = alive.clone();
        let advances_count = advances.clone();
        let handle = tokio::spawn(async move {
            run_change_watch_loop(
                alive,
                12u64,
                |_checkpoint, _timeout_ms| async {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    Ok(None)
                },
                move |_: ()| {
                    advances_count.fetch_add(1, Ordering::SeqCst);
                    true
                },
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        stop.store(false, Ordering::Release);
        let result: Result<(), String> = handle.await.expect("watch loop");
        assert!(result.is_ok());
        assert_eq!(advances.load(Ordering::SeqCst), 0);
    }
}
