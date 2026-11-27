use super::*;
use crate::domain::{
    market::TickValuePerShare,
    units::{ContractSize, Lots},
};

const BUYER: AgentId = AgentId::new(0);
const SELLER: AgentId = AgentId::new(1);
const OTHER: AgentId = AgentId::new(2);
const FEES: AgentId = AgentId::new(3);
const FUNDING: AgentId = AgentId::new(4);
const TODAY: MarketStep = MarketStep::new(1);
const TOMORROW: MarketStep = MarketStep::new(2);
const ID: ContractId = ContractId::new(0);
const OVERFLOW: AccountingError = AccountingError::Domain(DomainError::Overflow);

fn money(value: i64) -> Money {
    Money::new(CurrencyId::new(*b"USD").unwrap(), CashUnits::new(value))
}

fn foreign() -> Money {
    Money::new(CurrencyId::new(*b"EUR").unwrap(), CashUnits::new(1))
}

fn portfolio(lots: i64, cash: i64) -> Portfolio {
    Portfolio {
        net_lots: NetLots::new(lots),
        cash: money(cash),
    }
}

fn instrument(lot_micros: u32) -> InstrumentSpec {
    InstrumentSpec {
        tick_value_per_share: TickValuePerShare::new(money(1)).unwrap(),
        lot_size: LotSize::new(lot_micros).unwrap(),
    }
}

fn ledger() -> Ledger {
    Ledger::new(
        instrument(1_000_000),
        &[
            portfolio(2, 100),
            portfolio(-2, -100),
            portfolio(0, 0),
            portfolio(0, 0),
            portfolio(0, 0),
        ],
        FEES,
        FUNDING,
    )
    .unwrap()
}

fn price(ticks: u64) -> PriceTicks {
    PriceTicks::new(ticks).unwrap()
}

fn fill(buyer: AgentId, seller: AgentId) -> Fill {
    Fill::new(buyer, seller, Lots::new(1).unwrap(), price(15)).unwrap()
}

fn commission() -> OneWayCommissionRate {
    OneWayCommissionRate::new(0.1).unwrap()
}

fn make_call(id: u64, premium: Money) -> OptionContract {
    OptionContract::new(
        ContractId::new(id),
        BUYER,
        SELLER,
        ContractSize::new(2).unwrap(),
        price(10),
        PremiumPerShare::new(premium).unwrap(),
        TODAY,
        TOMORROW,
    )
    .unwrap()
}

#[test]
fn positions_and_cash_reconcile_for_fractional_lots_and_rounded_fees() {
    let mut ledger = Ledger::new(
        instrument(500_000),
        &[
            portfolio(0, 0),
            portfolio(2, 0),
            portfolio(0, 0),
            portfolio(0, 0),
            portfolio(0, 0),
        ],
        FEES,
        FUNDING,
    )
    .unwrap();

    ledger
        .book_fill(TODAY, fill(BUYER, SELLER), commission())
        .unwrap();
    // 15 * 0.5 rounds to 8 units, so the one-way fee of 0.8 rounds to 1.
    assert_eq!(ledger.portfolio(BUYER), Some(&portfolio(1, -9)));
    assert_eq!(ledger.portfolio(SELLER), Some(&portfolio(1, 7)));
    assert_eq!(ledger.portfolio(FEES), Some(&portfolio(0, 2)));
    assert!(ledger.check_conservation().is_ok());
    assert_eq!(
        ledger.pnl(BUYER, price(15), price(20), &BTreeMap::new()),
        Ok(money(1))
    );
    ledger.portfolios[0].net_lots = NetLots::new(0);
    assert_eq!(
        ledger.check_conservation(),
        Err(AccountingError::Conservation)
    );
}

#[test]
fn invalid_openings_and_snapshots_are_rejected() {
    let mut ledger = ledger();
    let mut opening = ledger.portfolios().to_vec();
    assert!(matches!(
        Ledger::new(ledger.instrument, &opening, FEES, FEES),
        Err(AccountingError::InvalidOpening)
    ));
    assert!(matches!(
        Ledger::new(ledger.instrument, &opening, AgentId::new(100), FUNDING),
        Err(AccountingError::InvalidOpening)
    ));
    let valid_opening = opening.clone();
    opening[0].cash = foreign();
    assert!(matches!(
        Ledger::new(ledger.instrument, &opening, FEES, FUNDING),
        Err(AccountingError::InvalidOpening)
    ));
    let rate = FundingRate::new(-0.02).unwrap();
    let day_count = TradingDays::new(252);
    for (snapshot, day_count) in [
        (&opening[..0], day_count),
        (&opening[..], day_count),
        (&valid_opening[..], TradingDays::new(0)),
    ] {
        assert_eq!(
            ledger.accrue_funding(TODAY, snapshot, rate, day_count),
            Err(AccountingError::InvalidFundingSnapshot)
        );
    }
}

#[test]
fn errors_leave_postings_and_balances_unchanged() {
    let mut ledger = ledger();
    let opening = ledger.portfolios().to_vec();
    let initial_postings = ledger.postings().len();

    assert_eq!(
        ledger.book_fill(TODAY, fill(FEES, SELLER), commission()),
        Err(AccountingError::ReservedAccount(FEES))
    );
    let missing = AgentId::new(7);
    assert_eq!(
        ledger.book_fill(TODAY, fill(BUYER, missing), commission()),
        Err(AccountingError::UnknownAgent(missing))
    );
    assert_eq!(
        ledger.issue_call(make_call(0, foreign())),
        Err(AccountingError::Domain(DomainError::CurrencyMismatch))
    );
    let oversized = Fill::new(BUYER, SELLER, Lots::new(u64::MAX).unwrap(), price(1)).unwrap();
    assert_eq!(
        ledger.book_fill(TODAY, oversized, commission()),
        Err(OVERFLOW)
    );
    assert_eq!(ledger.portfolios(), opening);
    assert_eq!(ledger.postings().len(), initial_postings);

    ledger.portfolios[1].net_lots = NetLots::new(i64::MIN);
    assert_eq!(
        ledger.book_fill(TODAY, fill(BUYER, SELLER), commission()),
        Err(OVERFLOW)
    );
    assert_eq!(ledger.portfolio(BUYER), Some(&opening[0]));
    assert_eq!(ledger.postings().len(), initial_postings);
}

#[test]
fn funding_is_signed_and_rolls_back_on_a_later_overflow() {
    let mut ledger = ledger();
    let opening = ledger.portfolios().to_vec();
    let rate = FundingRate::new(-0.06).unwrap();
    ledger
        .accrue_funding(TODAY, &opening, rate, TradingDays::new(2))
        .unwrap();
    assert_eq!(ledger.portfolio(BUYER), Some(&portfolio(2, 97)));
    assert_eq!(ledger.portfolio(SELLER), Some(&portfolio(-2, -97)));
    assert_eq!(ledger.portfolio(FUNDING), Some(&portfolio(0, 0)));
    assert_eq!(ledger.postings().len(), opening.len() + 2);

    ledger.portfolios[1].cash = money(i64::MAX);
    let current = ledger.portfolios().to_vec();
    let count = ledger.postings().len();
    assert_eq!(
        ledger.accrue_funding(TOMORROW, &opening, rate, TradingDays::new(2)),
        Err(OVERFLOW)
    );
    assert_eq!(ledger.portfolios(), current);
    assert_eq!(ledger.postings().len(), count);
}

#[test]
fn a_call_settles_once_and_cannot_be_reissued() {
    let mut ledger = ledger();
    let call = make_call(0, money(1));
    ledger.issue_call(call).unwrap();
    assert_eq!(ledger.active_call(SELLER), Some(&call));
    assert_eq!(ledger.active_call(BUYER), None);
    assert_eq!(
        ledger.issue_call(call),
        Err(AccountingError::DuplicateContract(ID))
    );
    let overlap = make_call(1, money(1));
    assert_eq!(
        ledger.issue_call(overlap),
        Err(AccountingError::ActiveSeller(SELLER))
    );

    let yesterday = MarketStep::new(0);
    assert_eq!(
        ledger.book_fill(yesterday, fill(BUYER, SELLER), commission()),
        Err(AccountingError::BackdatedPosting(yesterday))
    );
    assert_eq!(
        ledger.settle_expiring(MarketStep::new(3), price(20)),
        Err(AccountingError::OverdueContract(ID))
    );
    assert_eq!(ledger.settle_expiring(TOMORROW, price(20)), Ok(1));
    assert_eq!(ledger.portfolio(BUYER), Some(&portfolio(2, 118)));
    assert_eq!(ledger.portfolio(SELLER), Some(&portfolio(-2, -118)));
    assert_eq!(ledger.settle_expiring(TOMORROW, price(20)), Ok(0));

    let same_id = OptionContract::new(
        ID,
        BUYER,
        SELLER,
        call.size(),
        call.strike(),
        call.premium_per_share(),
        TOMORROW,
        MarketStep::new(3),
    )
    .unwrap();
    assert_eq!(
        ledger.issue_call(same_id),
        Err(AccountingError::DuplicateContract(ID))
    );
    assert!(ledger.check_conservation().is_ok());
}

#[test]
fn transaction_error_restores_positions_and_option_registry() {
    let mut ledger = ledger();
    let opening = ledger.portfolios().to_vec();
    let call = make_call(0, money(1));
    let result = ledger.transact(|ledger| {
        ledger.issue_call(call)?;
        ledger.book_fill(TODAY, fill(BUYER, SELLER), commission())?;
        ledger.settle_expiring(TOMORROW, price(10))?;
        Err::<(), _>(AccountingError::InvalidPosting)
    });
    assert_eq!(result, Err(AccountingError::InvalidPosting));
    assert_eq!(ledger.postings().len(), opening.len());
    assert_eq!(ledger.portfolios(), opening);
    assert_eq!(ledger.active_calls().count(), 0);
    assert_eq!(ledger.issue_call(call), Ok(()));
}

#[test]
fn terminal_pnl_uses_explicit_marks_for_both_counterparties() {
    let mut ledger = ledger();
    let start = price(10);
    let end = price(15);
    ledger.issue_call(make_call(0, money(2))).unwrap();
    let replayed = Ledger::replay(ledger.instrument, FEES, FUNDING, ledger.postings()).unwrap();
    let mark = PremiumPerShare::new(money(6)).unwrap();
    let marks = BTreeMap::from([(ID, mark)]);
    assert_eq!(replayed.pnl(BUYER, start, end, &marks), Ok(money(18)));
    assert_eq!(replayed.pnl(SELLER, start, end, &marks), Ok(money(-18)));
    assert_eq!(replayed.pnl(OTHER, start, end, &marks), Ok(money(0)));
    assert_eq!(
        replayed.pnl(BUYER, start, end, &BTreeMap::new()),
        Err(AccountingError::MissingMark(ID))
    );
    let unknown = ContractId::new(1);
    assert_eq!(
        replayed.pnl(BUYER, start, end, &BTreeMap::from([(unknown, mark)])),
        Err(AccountingError::UnknownMark(unknown))
    );
    assert_eq!(
        replayed.pnl(
            BUYER,
            start,
            end,
            &BTreeMap::from([(ID, PremiumPerShare::new(foreign()).unwrap())]),
        ),
        Err(AccountingError::Domain(DomainError::CurrencyMismatch))
    );
    assert_eq!(
        replayed.pnl(BUYER, start, price(u64::MAX), &marks),
        Err(OVERFLOW)
    );
}

#[test]
fn replay_rejects_inconsistent_postings() {
    let mut ledger = ledger();
    let opening_count = ledger.postings().len();
    ledger.issue_call(make_call(0, money(1))).unwrap();
    ledger
        .book_fill(TODAY, fill(BUYER, SELLER), commission())
        .unwrap();
    ledger.settle_expiring(TOMORROW, price(20)).unwrap();
    let journal = ledger.postings();

    let with_replacement = |index, replacement| {
        let mut entries = journal.to_vec();
        entries[index] = replacement;
        Ledger::replay(ledger.instrument, FEES, FUNDING, &entries).unwrap_err()
    };
    let kind_at = |index: usize, kind| Posting {
        kind,
        ..journal[index]
    };
    assert_eq!(
        with_replacement(0, journal[1]),
        AccountingError::InvalidOpening
    );
    assert_eq!(
        with_replacement(opening_count + 2, journal[0]),
        AccountingError::BackdatedPosting(MarketStep::new(0))
    );
    assert_eq!(
        with_replacement(
            opening_count + 1,
            kind_at(opening_count + 1, journal[0].kind)
        ),
        AccountingError::InvalidOpening
    );
    let fill_posting = |notional, fee_per_side, fee_account| PostingKind::Fill {
        fill: fill(BUYER, SELLER),
        notional,
        fee_per_side,
        fee_account,
    };
    assert_eq!(
        with_replacement(
            opening_count + 1,
            kind_at(opening_count + 1, fill_posting(money(16), money(2), FEES))
        ),
        AccountingError::InvalidPosting
    );
    assert_eq!(
        with_replacement(
            opening_count + 1,
            kind_at(opening_count + 1, fill_posting(money(15), money(-2), FEES))
        ),
        AccountingError::InvalidPosting
    );
    assert_eq!(
        with_replacement(
            opening_count + 1,
            kind_at(
                opening_count + 1,
                fill_posting(money(15), money(2), FUNDING)
            )
        ),
        AccountingError::InvalidPosting
    );
    assert_eq!(
        with_replacement(
            opening_count,
            kind_at(
                opening_count,
                PostingKind::OptionIssue {
                    contract: make_call(0, money(1)),
                    premium: money(3)
                }
            )
        ),
        AccountingError::InvalidPosting
    );
    assert_eq!(
        with_replacement(
            opening_count + 1,
            kind_at(
                opening_count + 1,
                PostingKind::Funding {
                    agent: BUYER,
                    funding_account: FEES,
                    interest: money(1)
                }
            )
        ),
        AccountingError::InvalidPosting
    );
    assert_eq!(
        with_replacement(
            opening_count + 2,
            kind_at(
                opening_count + 2,
                PostingKind::OptionSettlement {
                    contract: make_call(0, money(1)),
                    close: price(20),
                    payoff: money(21)
                }
            )
        ),
        AccountingError::InvalidSettlement(ID)
    );
}

#[test]
fn invalid_rotation_pairs_and_step_overflow_are_rejected() {
    let mut ledger = ledger();
    let size = ContractSize::new(1).unwrap();
    let premium = PremiumPerShare::new(money(1)).unwrap();
    assert!(matches!(
        Rotation::new(vec![(SELLER, SELLER)], size, CALL_TENOR),
        Err(AccountingError::Domain(DomainError::SameCounterparty))
    ));
    assert!(matches!(
        Rotation::new(vec![(BUYER, SELLER), (OTHER, SELLER)], size, CALL_TENOR),
        Err(AccountingError::ActiveSeller(SELLER))
    ));
    assert!(matches!(
        Rotation::new(vec![(BUYER, SELLER)], size, TradingDays::new(0)),
        Err(AccountingError::Domain(DomainError::InvalidValue(
            "call tenor"
        )))
    ));
    let rotation = Rotation::new(vec![(BUYER, SELLER)], size, CALL_TENOR).unwrap();
    assert_eq!(
        rotation.issue_due(
            &mut ledger,
            MarketStep::new(u32::MAX - 15),
            price(20),
            premium
        ),
        Err(OVERFLOW)
    );
    assert_eq!(
        rotation.issue_due(&mut ledger, TODAY, price(20), premium),
        Ok(0)
    );
}

#[test]
fn errors_describe_the_failed_operation() {
    let sample = [
        (AccountingError::Domain(DomainError::Overflow), "overflow"),
        (AccountingError::UnknownAgent(BUYER), "unknown agent"),
        (AccountingError::ReservedAccount(FEES), "reserved account"),
        (AccountingError::InvalidOpening, "opening portfolios"),
        (AccountingError::InvalidPosting, "posting"),
        (AccountingError::BackdatedPosting(TODAY), "backdated"),
        (AccountingError::DuplicateContract(ID), "already issued"),
        (AccountingError::ActiveSeller(SELLER), "active call"),
        (AccountingError::OverdueContract(ID), "missed its expiry"),
        (AccountingError::InvalidSettlement(ID), "invalid settlement"),
        (AccountingError::InvalidFundingSnapshot, "opening snapshot"),
        (AccountingError::UnknownMark(ID), "inactive contract"),
        (AccountingError::MissingMark(ID), "missing mark"),
        (AccountingError::Conservation, "reconcile"),
    ];
    for (error, description) in sample {
        assert!(error.to_string().contains(description));
    }
}

#[test]
fn rotation_rolls_back_if_a_later_counterparty_is_missing() {
    let mut ledger = ledger();
    let size = ContractSize::new(1).unwrap();
    let premium = PremiumPerShare::new(money(1)).unwrap();
    let missing = AgentId::new(50);
    let rotation = Rotation::new(
        vec![(BUYER, SELLER), (missing, OTHER)],
        size,
        TradingDays::new(1),
    )
    .unwrap();
    let opening = ledger.portfolios().to_vec();
    assert_eq!(
        rotation.issue_due(&mut ledger, TODAY, price(20), premium),
        Err(AccountingError::UnknownAgent(missing))
    );
    assert_eq!(ledger.portfolios(), opening);
    assert_eq!(ledger.postings().len(), opening.len());
    assert!(ledger.active_call(SELLER).is_none());
    let replacement = Rotation::new(vec![(BUYER, SELLER)], size, TradingDays::new(1)).unwrap();
    assert_eq!(
        replacement.issue_due(&mut ledger, TODAY, price(20), premium),
        Ok(1)
    );
}

#[test]
fn reserve_accounts_must_open_without_stock() {
    let ledger = ledger();
    for (reserve, lots) in [(FEES, 2), (FUNDING, -2)] {
        let mut opening = ledger.portfolios().to_vec();
        opening[agent_index(reserve)].net_lots = NetLots::new(lots);
        assert!(matches!(
            Ledger::new(ledger.instrument, &opening, FEES, FUNDING),
            Err(AccountingError::InvalidOpening)
        ));

        let mut postings = ledger.postings().to_vec();
        postings[agent_index(reserve)].kind = PostingKind::Opening {
            agent: reserve,
            portfolio: opening[agent_index(reserve)],
        };
        assert!(matches!(
            Ledger::replay(ledger.instrument, FEES, FUNDING, &postings),
            Err(AccountingError::InvalidOpening)
        ));
    }
}

#[test]
fn pnl_nets_wide_stock_and_option_marks_before_converting_to_money() {
    let opening = [
        portfolio(i64::MAX, 0),
        portfolio(i64::MIN, 0),
        portfolio(0, 0),
        portfolio(0, 0),
        portfolio(0, 0),
    ];
    let mut ledger = Ledger::new(instrument(1_000_000), &opening, FEES, FUNDING).unwrap();
    let no_marks = BTreeMap::new();
    assert_eq!(
        ledger.pnl(BUYER, price(2), price(2), &no_marks),
        Ok(money(0))
    );
    assert_eq!(
        ledger.pnl(BUYER, price(2), price(3), &no_marks),
        Ok(money(i64::MAX))
    );
    assert_eq!(
        ledger.pnl(SELLER, price(2), price(3), &no_marks),
        Ok(money(i64::MIN))
    );

    let call = OptionContract::new(
        ID,
        SELLER,
        BUYER,
        ContractSize::new(2).unwrap(),
        price(2),
        PremiumPerShare::new(money(0)).unwrap(),
        TODAY,
        TOMORROW,
    )
    .unwrap();
    ledger.issue_call(call).unwrap();
    let marks = BTreeMap::from([(ID, PremiumPerShare::new(money(i64::MAX)).unwrap())]);
    assert_eq!(
        ledger.pnl(BUYER, price(2), price(3), &marks),
        Ok(money(-i64::MAX))
    );
    assert_eq!(
        ledger.pnl(SELLER, price(2), price(3), &marks),
        Ok(money(i64::MAX - 1))
    );
}
