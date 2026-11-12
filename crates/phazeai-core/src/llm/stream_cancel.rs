//! Stop reading a provider's response once nobody is listening.
//!
//! Each provider reads its streamed HTTP response on a spawned task and
//! forwards events through an unbounded channel. When the agent is stopped it
//! drops the receiving end, but a task that only waits on the next network
//! chunk never notices: the connection stays open and the provider keeps
//! generating (and billing) until it finishes on its own. Reading through
//! [`next_unless_dropped`] ends the task, and with it the HTTP response, within
//! [`DROP_CHECK_INTERVAL`] of the receiver going away.

use crate::llm::traits::StreamEvent;
use futures::channel::mpsc::UnboundedSender;
use futures::{Stream, StreamExt};
use std::time::Duration;

/// How often a reader waiting on a quiet stream checks for a dropped receiver.
pub(crate) const DROP_CHECK_INTERVAL: Duration = Duration::from_millis(200);

/// The next item of `stream`, or `None` when the stream ends or `tx`'s
/// receiver has been dropped. Returning `None` lets the caller's read loop end
/// and drop the stream, which closes the HTTP response.
pub(crate) async fn next_unless_dropped<S>(
    stream: &mut S,
    tx: &UnboundedSender<StreamEvent>,
) -> Option<S::Item>
where
    S: Stream + Unpin,
{
    loop {
        if tx.is_closed() {
            return None;
        }
        // `StreamExt::next` is cancel-safe: dropping it between polls loses
        // nothing, so waking up to check the channel cannot skip a chunk.
        tokio::select! {
            item = stream.next() => return item,
            _ = tokio::time::sleep(DROP_CHECK_INTERVAL) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;

    #[tokio::test]
    async fn passes_items_through_while_the_receiver_is_held() {
        let (tx, _rx) = mpsc::unbounded();
        let mut stream = futures::stream::iter(vec![1, 2, 3]);
        let mut seen = Vec::new();
        while let Some(n) = next_unless_dropped(&mut stream, &tx).await {
            seen.push(n);
        }
        assert_eq!(seen, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn stops_at_once_when_the_receiver_is_gone() {
        let (tx, rx) = mpsc::unbounded();
        drop(rx);
        let mut stream = futures::stream::iter(vec![1, 2, 3]);
        assert_eq!(next_unless_dropped(&mut stream, &tx).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_stream_is_abandoned_soon_after_the_receiver_drops() {
        let (tx, rx) = mpsc::unbounded();
        // A stream that never yields, like a provider still thinking.
        let mut stream = futures::stream::pending::<u8>();
        let reader = tokio::spawn(async move { next_unless_dropped(&mut stream, &tx).await });
        tokio::time::sleep(DROP_CHECK_INTERVAL * 3).await;
        assert!(
            !reader.is_finished(),
            "must keep waiting while someone listens"
        );
        drop(rx);
        tokio::time::sleep(DROP_CHECK_INTERVAL * 2).await;
        assert_eq!(reader.await.unwrap(), None);
    }
}
