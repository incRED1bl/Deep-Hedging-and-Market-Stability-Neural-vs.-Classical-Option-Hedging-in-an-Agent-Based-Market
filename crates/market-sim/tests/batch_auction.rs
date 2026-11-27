use market_sim::domain::market::auction::{AuctionError, clear_auction};
use market_sim::domain::market::{Fill, Order, Side};
use market_sim::domain::units::{AgentId, Lots, MarketStep, PriceTicks};

fn price(ticks: u64) -> PriceTicks {
    PriceTicks::new(ticks).unwrap()
}

fn order(agent: u32, side: Side, lots: u64, limit: u64) -> Order {
    Order::new(
        AgentId::new(agent),
        side,
        Lots::new(lots).unwrap(),
        price(limit),
    )
}

fn fill(buyer: u32, seller: u32, lots: u64, ticks: u64) -> Fill {
    Fill::new(
        AgentId::new(buyer),
        AgentId::new(seller),
        Lots::new(lots).unwrap(),
        price(ticks),
    )
    .unwrap()
}

#[test]
fn one_cross_executes_at_the_prior_close_inside_the_gap() {
    let orders = [order(1, Side::Buy, 3, 102), order(2, Side::Sell, 1, 99)];
    let cleared = clear_auction(MarketStep::new(1), price(100), &orders).unwrap();

    assert_eq!(cleared.close, price(100));
    assert_eq!(cleared.traded_lots, 1);
    assert_eq!(cleared.fills, [fill(1, 2, 1, 100)]);
}

#[test]
fn volume_has_priority_over_proximity_to_the_prior_close() {
    let orders = [
        order(1, Side::Buy, 4, 105),
        order(2, Side::Buy, 2, 101),
        order(3, Side::Sell, 1, 99),
        order(4, Side::Sell, 3, 104),
    ];
    let cleared = clear_auction(MarketStep::new(1), price(100), &orders).unwrap();

    assert_eq!(cleared.close, price(104));
    assert_eq!(cleared.traded_lots, 4);
    assert_eq!(cleared.fills, [fill(1, 3, 1, 104), fill(1, 4, 3, 104)]);
}

#[test]
fn imbalance_has_priority_over_proximity_to_the_prior_close() {
    let orders = [
        order(1, Side::Buy, 4, 105),
        order(2, Side::Buy, 3, 100),
        order(3, Side::Sell, 4, 97),
    ];
    let cleared = clear_auction(MarketStep::new(1), price(99), &orders).unwrap();

    assert_eq!(cleared.close, price(101));
    assert_eq!(cleared.fills, [fill(1, 3, 4, 101)]);
}

#[test]
fn higher_buy_limit_has_priority_over_a_buy_at_the_clearing_price() {
    let orders = [
        order(1, Side::Buy, 2, 105),
        order(2, Side::Buy, 1, 105),
        order(3, Side::Buy, 6, 100),
        order(4, Side::Sell, 3, 100),
        order(5, Side::Sell, 20, 101),
    ];
    let cleared = clear_auction(MarketStep::new(2), price(100), &orders).unwrap();

    assert_eq!(cleared.close, price(100));
    assert_eq!(cleared.fills, [fill(1, 4, 2, 100), fill(2, 4, 1, 100)]);
}

#[test]
fn allocation_is_permutation_invariant_and_rotates_tied_orders() {
    let mut orders = [
        order(1, Side::Buy, 2, 105),
        order(2, Side::Buy, 2, 105),
        order(3, Side::Buy, 6, 103),
        order(4, Side::Sell, 1, 98),
        order(5, Side::Sell, 2, 99),
    ];
    let first = clear_auction(MarketStep::new(2), price(100), &orders).unwrap();
    let next = clear_auction(MarketStep::new(3), price(100), &orders).unwrap();
    orders.reverse();

    assert_eq!(
        first,
        clear_auction(MarketStep::new(2), price(100), &orders).unwrap()
    );
    assert_eq!(first.close, price(104));
    assert_eq!(first.traded_lots, 3);
    assert_eq!(
        first.fills,
        [fill(1, 4, 1, 104), fill(1, 5, 1, 104), fill(2, 5, 1, 104)]
    );
    assert_eq!(
        next.fills,
        [fill(2, 4, 1, 104), fill(2, 5, 1, 104), fill(1, 5, 1, 104)]
    );
}

#[test]
fn unfilled_books_keep_the_prior_close() {
    let bid = order(1, Side::Buy, 1, 98);
    let ask = order(2, Side::Sell, 1, 102);
    for orders in [&[][..], &[bid][..], &[ask][..], &[bid, ask][..]] {
        let cleared = clear_auction(MarketStep::new(1), price(100), orders).unwrap();

        assert_eq!(cleared.close, price(100));
        assert_eq!(cleared.traded_lots, 0);
        assert!(cleared.fills.is_empty());
    }
}

#[test]
fn supports_the_entire_positive_tick_and_lot_range() {
    let buy = order(1, Side::Buy, u64::MAX, u64::MAX);
    let sell = order(2, Side::Sell, u64::MAX, 1);
    let prior = u64::MAX - 1;
    let result = clear_auction(MarketStep::new(1), price(prior), &[buy, sell]).unwrap();

    assert_eq!(result.close, price(prior));
    assert_eq!(result.fills, [fill(1, 2, u64::MAX, prior)]);
}

#[test]
fn rejects_duplicate_agent_ids_and_side_quantity_overflow() {
    let bid = order(1, Side::Buy, 1, 100);
    let ask = order(1, Side::Sell, 1, 100);

    assert_eq!(
        clear_auction(MarketStep::new(1), price(100), &[bid, ask]),
        Err(AuctionError::DuplicateAgent(AgentId::new(1)))
    );
    for side in [Side::Buy, Side::Sell] {
        let orders = [order(1, side, u64::MAX, 100), order(2, side, 1, 100)];
        assert_eq!(
            clear_auction(MarketStep::new(1), price(100), &orders),
            Err(AuctionError::QuantityOverflow)
        );
    }
}
