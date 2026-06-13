use cb_hft::order::{NewOrderCommand, StrategyCommand, TimeInForce};
use cb_hft::ring::{CommandRingPair, RingError};
use cb_hft::types::{Price, Qty, Side, SymbolId};

fn new_order(strategy_order_id: u64) -> StrategyCommand {
    StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id: SymbolId(0),
        side: Side::Buy,
        price: Price(10_000),
        qty: Qty(100),
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id,
        signal_ts_ns: strategy_order_id,
    })
}

#[test]
fn spsc_command_ring_pair_exposes_independent_producer_and_consumer_handles() {
    let (mut producer, mut consumer) = CommandRingPair::new(2).unwrap();

    producer.push(new_order(1)).unwrap();
    producer.push(new_order(2)).unwrap();
    assert_eq!(producer.push(new_order(3)), Err(RingError::Full));

    assert_eq!(consumer.pop().unwrap().strategy_order_id(), 1);
    assert_eq!(consumer.pop().unwrap().strategy_order_id(), 2);
    assert_eq!(consumer.pop(), None);
}

#[test]
fn spsc_command_ring_pair_rejects_zero_capacity() {
    assert_eq!(CommandRingPair::new(0).err(), Some(RingError::ZeroCapacity));
}
