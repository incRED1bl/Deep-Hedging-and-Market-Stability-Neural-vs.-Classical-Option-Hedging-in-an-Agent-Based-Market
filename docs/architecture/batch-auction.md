# Batch auction

Each trading day accepts one net limit order per agent. A duplicate agent ID
rejects the batch. Order quantities are positive whole lots. Closing prices
are positive integer ticks.

For each candidate tick, buy demand includes orders whose limit is at or
above that tick. Sell supply includes orders whose limit is at or below it.
Executed volume is the smaller of demand and supply. The auction chooses a
tick in this order:

1. Largest executed volume.
2. Smallest absolute difference between eligible demand and supply.
3. Smallest distance from the previous close.
4. Lower tick.

The auction tests quoted ticks and the tick closest to the previous close
inside each gap between them. Both sides' eligible quantities stay constant
throughout a gap. If executed volume is zero, the previous close carries
forward and the batch emits no fills.

Buy orders at higher limits execute first. Sell orders at lower limits
execute first. Within a side and limit, orders are sorted by agent ID and
rotated left by the closing market step modulo the number of orders at that
limit. This advances the first agent each day for an unchanged price level.
Allocation takes the smaller of the remaining executable volume and the
quantities of the next buy and sell orders. Each fill carries the same
positive quantity for both counterparties and uses the selected price.

For example, a previous close of 100 and a buy for three lots up to 102
cross a sell for one lot at 99. Every tick from 99 through 102 has the same
volume and imbalance. The auction selects 100 and fills one lot. The buy's
remaining two lots expire with the batch.

For `n` orders the auction sorts and sweeps the limit levels in
`O(n log n)` time and uses `O(n)` memory. Price gaps do not increase
the number of candidates.
