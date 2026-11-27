# Portfolio and option accounting

The ledger owns each agent's signed net lots and cash in the instrument's quote currency. The fee and funding accounts have distinct agent IDs, zero opening lots, and cash transfers only. The call buyer and seller are market participants. A seller manages one active call at a time. The same buyer can hold several calls.

## Trading day

| Operation | Cash and position changes |
| --- | --- |
| Book actual fills | The buyer pays the seller the traded notional and receives the matched lots. Both sides pay the same one-way fee to the fee account. |
| Accrue funding | Each participant receives signed interest from the funding account. A negative amount transfers cash to the funding account. |
| Settle expiring calls | The seller pays the buyer `max(close - strike, 0) × contract size`, using the instrument's tick value per share. The call closes after one settlement, including when the payoff is zero. |
| Issue scheduled calls | The buyer pays the seller the premium per share times the contract size. The new call takes the current close as its strike. The portfolio carries its prior cash and lots into the new contract. |

The caller captures opening cash before fills. Interest is simple annual rate divided by trading days per year, applied to those opening balances. An annual rate of 0.252 with 252 trading days yields a daily rate of 0.001. Premiums and payoffs booked at that day's close first affect the next day's opening cash. Fees and interest each round to the nearest cash unit with ties away from zero. Fill notional rounds once before the fee is applied. The quote account can carry a debit balance. Cash and lot constraints on orders belong to order admission.

Retrying an already funded step leaves balances and the journal unchanged while that step is current. When all participants earn zero interest after rounding, the ledger records one zero-interest funding posting so replay recognizes the completed step.

Rotation issues each ordered buyer and seller pair at its phase, calculated as the pair index modulo the configured tenor. The default tenor is 20 trading days, so 40 hedgers receive two initial calls per close across steps 0 through 19. Step 20 settles the phase-zero contracts, then issues their replacements. Every new call has a unique ID. The application settles expiries at the last close and skips new issuance there.

## Journal and terminal value

Record the instrument, reserve account IDs, opening close, terminal close and terminal per-share marks with the journal. The ledger's opening postings provide the portfolio baseline. The fill, fee, funding, premium and settlement postings contain the booked cash amounts and counterparties. Replay reconstructs portfolios and active contracts using those postings.

Replay rejects a second funding posting for the same participant and market step, even when other operations appear between the two postings. For older journals, replay can recover completed funding steps only where a funding posting exists.

A run's total P&L is the change in marked equity. Marked equity is cash plus signed stock value at the close plus the signed value of each active call. The opening baseline uses the initial cash and stock before the first call issuance. Calls that expire on the final close settle at that close and appear in cash. Calls with later expiries remain live. Supply one quote-currency per-share market or model mark for each live call. Value each call at that mark times its contract size, positive for the buyer and negative for the seller. Report completed-contract hedging errors using settlements through the final close. P&L uses signed 128-bit intermediate marks and converts the net result to the cash unit. Extreme inputs that exceed signed 128-bit arithmetic before lot scaling or during accumulation return an overflow error.

All cash and lots reconcile across the participant and reserve accounts. Market price changes affect marked equity. The cash journal records amounts actually transferred. A failed transaction discards its tentative postings and restores the previous balances and active contracts.

## Hand-calculated verification

The auction fixture starts a hedger with `$200` and two shares worth `$100` each. The hedger sells a call on two shares with a `$100` strike and a `$2.50` premium per share. A three-share buy order fills one share at `$100`. The buyer pays `$100.10` in total, the seller receives `$99.90`, and the fee account receives `$0.20`. The fill leaves the hedger with three shares. Interest on the hedger's `$205` opening cash rounds to `$0.21`, leaving `$105.11` of cash before settlement.

| Ending close | Call payoff | Hedger cash | Hedger stock value | Hedger P&L |
| ---: | ---: | ---: | ---: | ---: |
| $110.00 | $20.00 | $85.11 | $330.00 | $15.11 |
| $100.00 | $0.00 | $105.11 | $300.00 | $5.11 |
| $90.00 | $0.00 | $105.11 | $270.00 | -$24.89 |

The integration fixture replays each case from actual auction fills and cash postings. The separate Python/DL reference comparison remains pending under [issue #5](https://github.com/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market/issues/5).
