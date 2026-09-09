//! Batching for channel sends.
//!
//! Discovery and Tier 0 both report one repository at a time, and a channel send per repository
//! would pay the JSON serialization cost hundreds of times over. This collects them into batches
//! instead: at most [`BATCH_MAX`] items, or whatever arrives within [`BATCH_WINDOW`] of the first.
//!
//! `recv_timeout` **is** the timer. There is no timer thread and no `tokio::time` here, and the
//! absence is deliberate: the walker and rayon threads are plain OS threads with no runtime
//! handle, so driving this from an async task would put the blocking Tier 0 fan-out inside the
//! async runtime — exactly what `spawn_blocking` exists to prevent.

use std::{
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// Repositories per channel send.
///
/// A full batch of rows exceeds Tauri's 8192-byte direct-`eval` threshold and therefore takes the
/// queue-plus-`fetch` path instead. That is not a reason to shrink it: 8192 is a measured
/// crossover, not a cliff, and ducking under it would trade one round trip for twice as many
/// `eval`s queued onto the event-loop thread — while making the batch size a function of how long
/// the user's paths happen to be.
pub const BATCH_MAX: usize = 25;

/// How long a partial batch waits before it is flushed anyway.
pub const BATCH_WINDOW: Duration = Duration::from_millis(50);

/// Take the next batch: up to `max` items, or whatever has arrived within `window` of the first.
///
/// Blocks indefinitely for the **first** item and only then opens the window. That is the property
/// that matters: a walk descending a large barren directory produces nothing for seconds, and a
/// batcher whose clock started at the call would emit a stream of empty events through it.
///
/// Returns `None` only when the channel is disconnected *and* nothing is buffered, which is the
/// signal that the producer is done. The final partial batch therefore needs no special case — it
/// falls out of `Disconnected` arriving with items already in hand.
pub fn next_batch<T>(rx: &Receiver<T>, max: usize, window: Duration) -> Option<Vec<T>> {
    let mut batch = Vec::with_capacity(max.min(BATCH_MAX));
    batch.push(rx.recv().ok()?);

    let deadline = Instant::now() + window;
    while batch.len() < max {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match rx.recv_timeout(remaining) {
            Ok(item) => batch.push(item),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    Some(batch)
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread};

    use super::*;

    /// The only assertion here with an *upper* time bound, and it carries a 5x margin so a loaded
    /// agent cannot flake it. Every other timing assertion in this module is a lower bound.
    #[test]
    fn a_full_batch_returns_without_waiting_for_the_window() {
        let (tx, rx) = mpsc::channel();
        for item in 0..30 {
            tx.send(item).expect("receiver is alive");
        }

        let started = Instant::now();
        let batch = next_batch(&rx, BATCH_MAX, Duration::from_millis(2_000)).expect("a batch");

        assert_eq!(batch.len(), BATCH_MAX);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "a full batch must not wait out the window: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_partial_batch_is_flushed_when_the_window_expires() {
        let (tx, rx) = mpsc::channel();
        for item in 0..3 {
            tx.send(item).expect("receiver is alive");
        }

        let started = Instant::now();
        let batch = next_batch(&rx, BATCH_MAX, Duration::from_millis(50)).expect("a batch");

        assert_eq!(batch, vec![0, 1, 2]);
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    /// The producer finishing mid-window must not lose what it already sent.
    #[test]
    fn the_final_partial_batch_survives_the_sender_dropping() {
        let (tx, rx) = mpsc::channel();
        for item in 0..3 {
            tx.send(item).expect("receiver is alive");
        }
        drop(tx);

        assert_eq!(
            next_batch(&rx, BATCH_MAX, BATCH_WINDOW),
            Some(vec![0, 1, 2])
        );
        assert_eq!(next_batch(&rx, BATCH_MAX, BATCH_WINDOW), None);
    }

    #[test]
    fn an_empty_disconnected_channel_yields_none() {
        let (tx, rx) = mpsc::channel::<u8>();
        drop(tx);

        assert_eq!(next_batch(&rx, BATCH_MAX, BATCH_WINDOW), None);
    }

    /// The window opens at the first item, not at the call. Without this the batcher would return
    /// an empty batch for every quiet stretch of a walk.
    #[test]
    fn the_window_starts_at_the_first_item() {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            tx.send(7).expect("receiver is alive");
        });

        let batch = next_batch(&rx, BATCH_MAX, Duration::from_millis(50)).expect("a batch");

        assert_eq!(batch, vec![7], "the wait for the first item is unbounded");
    }
}
