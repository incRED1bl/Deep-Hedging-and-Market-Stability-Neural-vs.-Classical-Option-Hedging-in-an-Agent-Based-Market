use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::application::market_day::PositionAggregate;
use crate::application::simulation::{DayRecord, JournalSink, RunMetadata};
use crate::domain::accounting::ledger::{Posting, PostingKind};
use crate::domain::accounting::{OptionContract, PremiumPerShare};
use crate::domain::units::{AgentId, ContractId, Money};

/// Publish each completed day with an atomic hard link in a fresh directory.
pub struct JsonDirectoryJournal {
    directory: PathBuf,
}

impl JsonDirectoryJournal {
    pub fn new(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir(&directory)?;
        Ok(Self { directory })
    }
}

impl JournalSink for JsonDirectoryJournal {
    fn publish(&mut self, day: &DayRecord) -> io::Result<()> {
        let committed = self
            .directory
            .join(format!("{:010}.json", day.market.state.step.get()));
        let mut bytes = serde_json::to_vec(&day_json(day)).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let staged = committed.with_extension("staged");
        let mut file = File::create_new(&staged)?;
        let written = file.write_all(&bytes);
        drop(file);
        let publish = written.and_then(|()| fs::hard_link(&staged, committed));
        let _ = fs::remove_file(staged);
        publish
    }
}

fn day_json(day: &DayRecord) -> Value {
    let state = day.market.state;
    let positions = day.market.positions;
    let terminal = day.terminal.as_ref().map(|terminal| {
        json!({
            "live_call_marks": marks_json(&terminal.live_call_marks),
            "lifetime_hedger_pnl": pnl_json(&terminal.lifetime_hedger_pnl),
            "observed_hedger_pnl": pnl_json(&terminal.observed_hedger_pnl),
        })
    });
    let postings: Vec<_> = day.postings.iter().map(posting_json).collect();
    json!({
        "schema_version": 1,
        "step": state.step.get(),
        "included_in_metrics": day.included_in_metrics,
        "close_ticks": state.close.get(),
        "gross_return": state.gross_return.get(),
        "annualized_volatility": state.annualized_volatility.get(),
        "traded_lots": day.market.traded_lots,
        "positions": {
            "fundamentalists": aggregate(positions.fundamentalists),
            "trend_traders": aggregate(positions.trend_traders),
            "noise_traders": aggregate(positions.noise_traders),
            "hedgers": aggregate(positions.hedgers),
        },
        "postings": postings,
        "metadata": day.metadata.map(metadata_json),
        "burn_in_marks": day.burn_in_marks.as_ref().map(marks_json),
        "terminal": terminal,
    })
}

fn marks_json(marks: &BTreeMap<ContractId, PremiumPerShare>) -> Vec<Value> {
    marks
        .iter()
        .map(|(id, mark)| json!({"contract": id.get(), "per_share": cash(mark.get())}))
        .collect()
}

fn pnl_json(values: &[(AgentId, Money)]) -> Vec<Value> {
    values
        .iter()
        .map(|(agent, pnl)| json!({"agent": agent.get(), "pnl": cash(*pnl)}))
        .collect()
}

fn metadata_json(metadata: RunMetadata) -> Value {
    let scenario = metadata.scenario;
    let instrument = scenario.instrument;
    json!({
        "seed": scenario.seed,
        "burn_in_days": scenario.burn_in.get(),
        "observed_days": scenario.observed_days.get(),
        "one_way_commission": scenario.commission.get(),
        "annual_funding_rate": scenario.funding_rate.get(),
        "call": {
            "size_shares": scenario.call.size.get(),
            "maturity_days": scenario.call.maturity.get(),
        },
        "population": {
            "fundamentalists": scenario.population.fundamentalists,
            "trend_traders": scenario.population.trend_traders,
            "noise_traders": scenario.population.noise_traders,
            "hedgers": scenario.population.hedgers,
        },
        "limits": {
            "max_order_lots": scenario.limits.max_order_lots().get(),
            "max_abs_position_lots": scenario.limits.max_abs_position_lots().get(),
        },
        "trading_days_per_year": scenario.trading_days_per_year.get(),
        "cash_decimal_places": scenario.cash_decimal_places.get(),
        "tick_value_per_share": cash(instrument.tick_value_per_share.get()),
        "lot_size_micros": instrument.lot_size.get(),
        "fee_account": metadata.fee_account.get(),
        "funding_account": metadata.funding_account.get(),
        "call_buyer": metadata.call_buyer.get(),
    })
}

fn aggregate(cohort: PositionAggregate) -> Value {
    json!({"net_lots": cohort.net_lots.to_string(), "gross_lots": cohort.gross_lots.to_string()})
}

fn posting_json(posting: &Posting) -> Value {
    match posting.kind {
        PostingKind::Opening { agent, portfolio } => json!({
            "type": "opening",
            "agent": agent.get(),
            "net_lots": portfolio.net_lots.get(),
            "cash": cash(portfolio.cash),
        }),
        PostingKind::Fill {
            fill,
            notional,
            fee_per_side,
            fee_account,
        } => json!({
            "type": "fill",
            "buyer": fill.buyer().get(),
            "seller": fill.seller().get(),
            "lots": fill.lots().get(),
            "price_ticks": fill.price().get(),
            "notional": cash(notional),
            "fee_per_side": cash(fee_per_side),
            "fee_account": fee_account.get(),
        }),
        PostingKind::Funding {
            agent,
            funding_account,
            interest,
        } => json!({
            "type": "funding",
            "agent": agent.get(),
            "funding_account": funding_account.get(),
            "interest": cash(interest),
        }),
        PostingKind::OptionIssue { contract, premium } => json!({
            "type": "call_issue",
            "contract": call(contract),
            "premium": cash(premium),
        }),
        PostingKind::OptionSettlement {
            contract,
            close,
            payoff,
        } => json!({
            "type": "call_settlement",
            "contract": call(contract),
            "close_ticks": close.get(),
            "payoff": cash(payoff),
        }),
    }
}

fn call(contract: OptionContract) -> Value {
    json!({
        "id": contract.id().get(),
        "buyer": contract.buyer().get(),
        "seller": contract.seller().get(),
        "size_shares": contract.size().get(),
        "strike_ticks": contract.strike().get(),
        "premium_per_share": cash(contract.premium_per_share().get()),
        "issued_at": contract.issued_at().get(),
        "expires_at": contract.expires_at().get(),
    })
}

fn cash(money: Money) -> Value {
    let currency = money.currency().get();
    let code =
        std::str::from_utf8(&currency).expect("currency identifiers contain uppercase ASCII");
    json!({"currency": code, "units": money.amount().get()})
}
