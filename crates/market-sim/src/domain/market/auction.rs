use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use super::{Fill, Order, Side};
use crate::domain::units::{AgentId, Lots, MarketStep, PriceTicks};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuctionError {
    DuplicateAgent(AgentId),
    QuantityOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionResult {
    pub close: PriceTicks,
    pub fills: Vec<Fill>,
    pub traded_lots: u64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct PriceRank {
    volume: u64,
    imbalance: Reverse<u64>,
    distance_from_prior: Reverse<u64>,
    price: Reverse<u64>,
}

/// Clears one net limit order per agent at a uniform price. With zero
/// executable volume, the previous close carries forward.
pub fn clear_auction(
    step: MarketStep,
    prior_close: PriceTicks,
    orders: &[Order],
) -> Result<AuctionResult, AuctionError> {
    let mut participants = BTreeSet::new();
    let mut levels = BTreeMap::<u64, (u64, u64)>::new();
    let mut buys = Vec::new();
    let mut sells = Vec::new();
    let mut total_demand = 0_u64;
    let mut total_supply = 0_u64;
    for order in orders {
        if !participants.insert(order.agent().get()) {
            return Err(AuctionError::DuplicateAgent(order.agent()));
        }
        let lots = order.lots().get();
        let (buy_lots, sell_lots) = levels.entry(order.limit().get()).or_default();
        match order.side() {
            Side::Buy => {
                total_demand = total_demand
                    .checked_add(lots)
                    .ok_or(AuctionError::QuantityOverflow)?;
                *buy_lots += lots;
                buys.push(order);
            }
            Side::Sell => {
                total_supply = total_supply
                    .checked_add(lots)
                    .ok_or(AuctionError::QuantityOverflow)?;
                *sell_lots += lots;
                sells.push(order);
            }
        }
    }

    let prior = prior_close.get();
    let mut best = price_rank(prior, 0, 0, prior);
    let mut demand = total_demand;
    let mut supply = 0;
    let mut levels = levels.into_iter().peekable();
    while let Some((price, (buy_lots, sell_lots))) = levels.next() {
        supply += sell_lots;
        best = best.max(price_rank(price, demand, supply, prior));
        demand -= buy_lots;
        if let Some(&(next_price, _)) = levels.peek()
            && next_price - price > 1
        {
            // Demand and supply are constant until the next quoted limit.
            let gap_price = prior.clamp(price + 1, next_price - 1);
            best = best.max(price_rank(gap_price, demand, supply, prior));
        }
    }

    let close = PriceTicks::new(best.price.0).expect("candidate price is positive");
    let fills = allocate(step, close, best.volume, &mut buys, &mut sells);
    Ok(AuctionResult {
        close,
        fills,
        traded_lots: best.volume,
    })
}

fn price_rank(price: u64, demand: u64, supply: u64, prior: u64) -> PriceRank {
    PriceRank {
        volume: demand.min(supply),
        imbalance: Reverse(demand.abs_diff(supply)),
        distance_from_prior: Reverse(price.abs_diff(prior)),
        price: Reverse(price),
    }
}

fn allocate(
    step: MarketStep,
    close: PriceTicks,
    mut unfilled: u64,
    buys: &mut [&Order],
    sells: &mut [&Order],
) -> Vec<Fill> {
    let mut fills = Vec::new();
    if unfilled == 0 {
        return fills;
    }

    buys.sort_unstable_by_key(|order| (Reverse(order.limit().get()), order.agent().get()));
    sells.sort_unstable_by_key(|order| (order.limit().get(), order.agent().get()));
    rotate_at_each_price(buys, step);
    rotate_at_each_price(sells, step);

    let mut buyer = 0;
    let mut seller = 0;
    let mut buy_lots = buys[buyer].lots().get();
    let mut sell_lots = sells[seller].lots().get();
    loop {
        let quantity = buy_lots.min(sell_lots).min(unfilled);
        let lots = Lots::new(quantity).expect("each match has positive remaining lots");
        fills.push(
            Fill::new(buys[buyer].agent(), sells[seller].agent(), lots, close)
                .expect("orders have unique counterparties"),
        );
        unfilled -= quantity;
        if unfilled == 0 {
            return fills;
        }

        buy_lots -= quantity;
        if buy_lots == 0 {
            buyer += 1;
            buy_lots = buys[buyer].lots().get();
        }
        sell_lots -= quantity;
        if sell_lots == 0 {
            seller += 1;
            sell_lots = sells[seller].lots().get();
        }
    }
}

fn rotate_at_each_price(orders: &mut [&Order], step: MarketStep) {
    for level in orders.chunk_by_mut(|left, right| left.limit() == right.limit()) {
        let offset = step.get() as usize % level.len();
        level.rotate_left(offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equally_distant_prices_prefer_the_lower_tick() {
        assert!(price_rank(99, 4, 4, 100) > price_rank(101, 4, 4, 100));
    }
}
