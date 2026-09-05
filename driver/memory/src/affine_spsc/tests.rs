extern crate std;

use super::*;

type Queue = AffineSpscQueue<u8, 3>;

#[test]
fn cursor_domain_wraps_without_changing_fifo_distance() {
    assert_eq!(Queue::cursor_distance(0, 3), 3);
    assert_eq!(Queue::advance(5), 0);
    assert_eq!(Queue::cursor_distance(5, 1), 2);
}

#[test]
fn only_one_endpoint_pair_owns_an_epoch() {
    let queue = Queue::new();
    let endpoints = queue.split();
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| queue.split()));
    assert!(duplicate.is_err());
    drop(endpoints);
    let reused = queue.split();
    drop(reused);
}

#[test]
fn full_and_empty_transitions_preserve_affine_values() {
    let queue = AffineSpscQueue::<u8, 2>::new();
    let (producer, consumer) = queue.split();
    producer.try_send(1).unwrap();
    producer.try_send(2).unwrap();
    assert_eq!(producer.try_send(3).unwrap_err().0, 3);
    assert_eq!(consumer.try_receive(), Ok(1));
    assert_eq!(consumer.try_receive(), Ok(2));
    assert_eq!(
        consumer.try_receive(),
        Err(AffineSpscTryReceiveError::Empty)
    );
}

#[test]
fn persistent_producer_resumes_one_consumer_after_an_empty_epoch() {
    let queue = Queue::new();
    let (producer, consumer) = queue.split();
    producer.try_send(1).unwrap();
    assert_eq!(consumer.try_receive(), Ok(1));
    drop(consumer);

    let resumed = producer.resume_consumer();
    producer.try_send(2).unwrap();
    assert_eq!(resumed.try_receive(), Ok(2));
    let duplicate =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| producer.resume_consumer()));
    assert!(duplicate.is_err());
    drop(resumed);
    drop(producer);

    let reused = queue.split();
    drop(reused);
}
