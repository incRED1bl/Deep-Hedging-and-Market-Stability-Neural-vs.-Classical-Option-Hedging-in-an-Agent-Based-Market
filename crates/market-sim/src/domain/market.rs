use super::hedging::TargetPosition;
use super::units::{
    AgentId, ContractSize, DomainError, LotSize, Lots, Money, NetLots, PriceTicks, Shares,
    rounded_cash,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickValuePerShare(Money);

impl TickValuePerShare {
    pub fn new(value: Money) -> Result<Self, DomainError> {
        if value.amount().get() <= 0 {
            return Err(DomainError::InvalidValue("TickValuePerShare"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> Money {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstrumentSpec {
    pub tick_value_per_share: TickValuePerShare,
    pub lot_size: LotSize,
}

impl InstrumentSpec {
    pub fn shares(self, lots: NetLots) -> Shares {
        Shares::from_lots(lots, self.lot_size)
    }

    /// Book the value of a stock fill at the instrument's cash precision.
    pub fn notional(self, price: PriceTicks, lots: Lots) -> Result<Money, DomainError> {
        let tick = self.tick_value_per_share.get();
        let numerator = i128::from(price.get())
            .checked_mul(i128::from(lots.get()))
            .and_then(|value| value.checked_mul(i128::from(self.lot_size.get())))
            .and_then(|value| value.checked_mul(i128::from(tick.amount().get())))
            .ok_or(DomainError::Overflow)?;
        rounded_cash(
            tick.currency(),
            numerator,
            i128::from(LotSize::MICROS_PER_SHARE),
        )
    }

    /// Calculate cash for a call's whole-share underlying size and a nonnegative price in ticks.
    pub fn option_notional(
        self,
        price_ticks: u64,
        size: ContractSize,
    ) -> Result<Money, DomainError> {
        let tick = self.tick_value_per_share.get();
        let numerator = i128::from(price_ticks)
            .checked_mul(i128::from(size.get()))
            .and_then(|value| value.checked_mul(i128::from(tick.amount().get())))
            .ok_or(DomainError::Overflow)?;
        rounded_cash(tick.currency(), numerator, 1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Order {
    agent: AgentId,
    side: Side,
    lots: Lots,
    limit: PriceTicks,
}

impl Order {
    pub const fn new(agent: AgentId, side: Side, lots: Lots, limit: PriceTicks) -> Self {
        Self {
            agent,
            side,
            lots,
            limit,
        }
    }

    pub const fn agent(self) -> AgentId {
        self.agent
    }

    pub const fn side(self) -> Side {
        self.side
    }

    pub const fn lots(self) -> Lots {
        self.lots
    }

    pub const fn limit(self) -> PriceTicks {
        self.limit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fill {
    buyer: AgentId,
    seller: AgentId,
    lots: Lots,
    price: PriceTicks,
}

impl Fill {
    pub fn new(
        buyer: AgentId,
        seller: AgentId,
        lots: Lots,
        price: PriceTicks,
    ) -> Result<Self, DomainError> {
        if buyer == seller {
            return Err(DomainError::SameCounterparty);
        }
        Ok(Self {
            buyer,
            seller,
            lots,
            price,
        })
    }

    pub const fn buyer(self) -> AgentId {
        self.buyer
    }

    pub const fn seller(self) -> AgentId {
        self.seller
    }

    pub const fn lots(self) -> Lots {
        self.lots
    }

    pub const fn price(self) -> PriceTicks {
        self.price
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderLimits {
    max_order_lots: Lots,
    max_abs_position_lots: Lots,
}

impl OrderLimits {
    pub fn new(max_order_lots: Lots, max_abs_position_lots: Lots) -> Result<Self, DomainError> {
        if max_abs_position_lots.get() > i64::MAX as u64 {
            return Err(DomainError::InvalidValue("absolute position limit"));
        }
        Ok(Self {
            max_order_lots,
            max_abs_position_lots,
        })
    }

    pub const fn max_order_lots(self) -> Lots {
        self.max_order_lots
    }

    pub const fn max_abs_position_lots(self) -> Lots {
        self.max_abs_position_lots
    }

    /// Convert an absolute share target into a risk-limited whole-lot order.
    pub fn hedge_order(
        self,
        instrument: InstrumentSpec,
        agent: AgentId,
        held: NetLots,
        target: TargetPosition,
        limit: PriceTicks,
    ) -> Result<Option<Order>, DomainError> {
        let opening_shares = instrument.shares(held);
        if target.target_shares == opening_shares {
            return self.limit_order(agent, held, 0, limit);
        }

        let (micros, fractional) = floor_target_micros(target.target_shares.get());
        let lot_micros = i128::from(instrument.lot_size.get());
        let lower = micros.div_euclid(lot_micros);
        let upper = lower + i128::from(fractional || micros.rem_euclid(lot_micros) != 0);
        let held_lots = i128::from(held.get());
        let cap = i128::from(self.max_abs_position_lots.get());

        // Target shares can be the floating-point image of a fractional lot boundary.
        // Recognize it when a single lot count maps to that target.
        let desired = if lower == upper {
            lower
        } else if held_lots <= lower {
            if upper <= cap && uniquely_represents(instrument, target.target_shares, upper as i64) {
                upper
            } else {
                lower
            }
        } else if lower >= -cap
            && uniquely_represents(instrument, target.target_shares, lower as i64)
        {
            lower
        } else {
            upper
        };
        // The target's direction relative to the published opening observation
        // also applies where nearby floating-point values alias integer lots.
        let requested = desired - held_lots;
        let requested = if target.target_shares > opening_shares {
            requested.max(0)
        } else {
            requested.min(0)
        };
        self.limit_order(agent, held, requested, limit)
    }

    /// Clip a signed whole-lot intention to the per-order and position limits.
    pub fn limit_order(
        self,
        agent: AgentId,
        held: NetLots,
        requested: i128,
        limit: PriceTicks,
    ) -> Result<Option<Order>, DomainError> {
        let position_limit = self.max_abs_position_lots.get();
        if held.get().unsigned_abs() > position_limit {
            return Err(DomainError::InvalidValue("opening position"));
        }

        let position_limit = i128::from(position_limit);
        let held = i128::from(held.get());
        let permitted = requested.clamp(-position_limit - held, position_limit - held);
        let quantity = permitted
            .unsigned_abs()
            .min(u128::from(self.max_order_lots.get()));
        let Ok(lots) = Lots::new(quantity as u64) else {
            return Ok(None);
        };
        let side = if permitted > 0 { Side::Buy } else { Side::Sell };
        Ok(Some(Order::new(agent, side, lots, limit)))
    }
}

/// Floor the exact binary64 target on the millionth-of-a-share scale.
fn floor_target_micros(shares: f64) -> (i128, bool) {
    // Even i64::MIN lots of the largest lot size have fewer than 2^96 micros.
    const LIMIT: i128 = 1 << 96;
    let scale = f64::from(LotSize::MICROS_PER_SHARE);
    let scaled = shares * scale;
    if scaled >= LIMIT as f64 {
        return (LIMIT, false);
    }
    if scaled <= -LIMIT as f64 {
        return (-LIMIT, false);
    }
    if scaled.fract() != 0.0 {
        // Nearest rounding cannot cross an exactly representable integer boundary here.
        return (scaled.floor() as i128, true);
    }
    // The fused operation recovers the error of the rounded product. It can represent
    // several whole micros at large targets, or move an integer result across zero.
    let error = shares.mul_add(scale, -scaled);
    (scaled as i128 + error.floor() as i128, error.fract() != 0.0)
}

fn uniquely_represents(instrument: InstrumentSpec, target: Shares, lots: i64) -> bool {
    if instrument.shares(NetLots::new(lots)) != target
        || [lots.checked_sub(1), lots.checked_add(1)]
            .into_iter()
            .flatten()
            .any(|neighbor| instrument.shares(NetLots::new(neighbor)) == target)
    {
        return false;
    }

    // Keep the boundary bracketed by the floats adjacent to the observation,
    // even when the integer-to-float conversion and scaling rounded separately.
    let boundary = i128::from(lots) * i128::from(instrument.lot_size.get());
    let (previous, fractional) = floor_target_micros(target.get().next_down());
    let (next, _) = floor_target_micros(target.get().next_up());
    (previous < boundary || (previous == boundary && !fractional)) && boundary <= next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::units::{CashUnits, CurrencyId};

    fn cash(amount: i64) -> Money {
        Money::new(CurrencyId::new(*b"USD").unwrap(), CashUnits::new(amount))
    }

    fn instrument(lot_size_micros: u32, tick_value: i64) -> InstrumentSpec {
        InstrumentSpec {
            lot_size: LotSize::new(lot_size_micros).unwrap(),
            tick_value_per_share: TickValuePerShare::new(cash(tick_value)).unwrap(),
        }
    }

    #[test]
    fn instrument_books_fractional_lots_and_checks_overflow() {
        let quarter = instrument(250_000, 1);
        let one = Lots::new(1).unwrap();
        let price = PriceTicks::new(2).unwrap();
        assert_eq!(quarter.shares(NetLots::new(-3)).get(), -0.75);
        assert_eq!(quarter.notional(price, one), Ok(cash(1)));
        let size = ContractSize::new(4).unwrap();
        assert_eq!(quarter.option_notional(0, size), Ok(cash(0)));
        assert_eq!(quarter.option_notional(2, size), Ok(cash(8)));
        assert!(TickValuePerShare::new(cash(0)).is_err());
        let large = instrument(u32::MAX, i64::MAX);
        let max_ticks = PriceTicks::new(u64::MAX).unwrap();
        let max_lots = Lots::new(u64::MAX).unwrap();
        assert_eq!(
            large.notional(max_ticks, max_lots),
            Err(DomainError::Overflow)
        );
        assert_eq!(
            large.option_notional(u64::MAX, size),
            Err(DomainError::Overflow)
        );
    }

    #[test]
    fn fractional_lot_boundaries_preserve_the_full_target_change() {
        let limits = OrderLimits::new(Lots::new(500).unwrap(), Lots::new(1000).unwrap()).unwrap();
        let hundredth = instrument(10_000, 1);
        let price = PriceTicks::new(100).unwrap();
        let agent = AgentId::new(3);
        let order = |held, target_shares| {
            limits.hedge_order(
                hundredth,
                agent,
                NetLots::new(held),
                TargetPosition { target_shares },
                price,
            )
        };
        let long = hundredth.shares(NetLots::new(205));
        let short = hundredth.shares(NetLots::new(-205));
        let almost_long = Shares::new(long.get().next_down()).unwrap();
        let almost_short = Shares::new(short.get().next_up()).unwrap();
        for (held, target, side, lots) in [
            (0, long, Side::Buy, 205),
            (200, long, Side::Buy, 5),
            (200, almost_long, Side::Buy, 4),
            (0, short, Side::Sell, 205),
            (-200, short, Side::Sell, 5),
            (-200, almost_short, Side::Sell, 4),
        ] {
            let placed = order(held, target).unwrap().unwrap();
            assert_eq!((placed.side(), placed.lots().get()), (side, lots));
        }
        assert_eq!(order(204, long).unwrap().unwrap().lots().get(), 1);
        assert_eq!(order(-204, short).unwrap().unwrap().lots().get(), 1);
        assert_eq!(order(205, long), Ok(None));
        assert_eq!(order(-205, short), Ok(None));
    }

    #[test]
    fn large_targets_use_exact_lot_boundaries_when_float_observations_alias() {
        let cap = Lots::new(i64::MAX as u64).unwrap();
        let limits = OrderLimits::new(cap, cap).unwrap();
        let unit = instrument(LotSize::MICROS_PER_SHARE, 1);
        let micro = instrument(1, 1);
        let price = PriceTicks::new(100).unwrap();
        let agent = AgentId::new(3);
        let order = |instrument, held, target| {
            limits.hedge_order(
                instrument,
                agent,
                NetLots::new(held),
                TargetPosition {
                    target_shares: Shares::new(target).unwrap(),
                },
                price,
            )
        };

        let boundary = 1_i64 << 54;
        let target = boundary as f64;
        let micro_held = (1_i64 << 43) * i64::from(LotSize::MICROS_PER_SHARE);
        let micro_target = ((1_u64 << 43) as f64).next_up();
        for (spec, held, target, expected) in [
            (unit, boundary - 2, target, 2),
            (unit, boundary - 2, target.next_down(), 0),
            (unit, boundary - 2, target.next_up(), 6),
            (unit, boundary + 1, target, 0),
            (micro, micro_held, micro_target, 1_953),
            (unit, i64::MAX - 1, f64::MAX, 1),
        ] {
            for (sign, side) in [(1_i64, Side::Buy), (-1_i64, Side::Sell)] {
                let placed = order(spec, held * sign, target * sign as f64).unwrap();
                assert_eq!(
                    placed.map(|placed| (placed.side(), placed.lots().get())),
                    (expected != 0).then_some((side, expected)),
                );
            }
        }
    }

    #[test]
    fn target_orders_truncate_change_and_respect_both_limits() {
        let max_order = Lots::new(3).unwrap();
        let max_abs = Lots::new(6).unwrap();
        assert!(OrderLimits::new(max_order, Lots::new(u64::MAX).unwrap()).is_err());
        let limits = OrderLimits::new(max_order, max_abs).unwrap();
        assert_eq!(limits.max_order_lots(), max_order);
        assert_eq!(limits.max_abs_position_lots(), max_abs);
        let tenth = instrument(100_000, 1);
        let agent = AgentId::new(3);
        let limit = PriceTicks::new(70).unwrap();
        let order = |held, target| {
            limits.hedge_order(
                tenth,
                agent,
                NetLots::new(held),
                TargetPosition {
                    target_shares: Shares::new(target).unwrap(),
                },
                limit,
            )
        };
        let buy = order(1, 0.3).unwrap().unwrap();
        assert_eq!(buy.lots().get(), 2);
        assert_eq!(buy.side(), Side::Buy);
        assert_eq!(order(1, 0.04), Ok(None));
        assert_eq!(order(5, f64::MAX).unwrap().unwrap().lots().get(), 1);
        let sell = order(2, -10.0).unwrap().unwrap();
        assert_eq!((sell.side(), sell.lots().get()), (Side::Sell, 3));
        assert_eq!(
            order(7, 0.0),
            Err(DomainError::InvalidValue("opening position"))
        );

        let large_limit = Lots::new(i64::MAX as u64).unwrap();
        let large_limits = OrderLimits::new(large_limit, large_limit).unwrap();
        let large_position = NetLots::new(i64::MAX - 254);
        assert_eq!(
            large_limits.hedge_order(
                tenth,
                agent,
                large_position,
                TargetPosition {
                    target_shares: tenth.shares(large_position)
                },
                limit,
            ),
            Ok(None),
        );

        let fill = Fill::new(agent, AgentId::new(4), buy.lots(), limit).unwrap();
        assert_eq!(fill.buyer(), agent);
        assert_eq!(fill.seller(), AgentId::new(4));
        assert_eq!(fill.price(), limit);
        assert_eq!(
            Fill::new(agent, agent, buy.lots(), limit),
            Err(DomainError::SameCounterparty)
        );
    }
}
