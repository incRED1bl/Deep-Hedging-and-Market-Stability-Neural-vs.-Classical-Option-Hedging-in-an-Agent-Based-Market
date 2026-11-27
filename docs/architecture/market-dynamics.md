# Market dynamics

The research fixture configures 60 fundamentalists, 50 trend traders, 50 noise
traders and 40 hedgers. Trading agents have dense IDs in that order. Each
background agent carries its own signal, and each hedger records its latest
absolute target. Opening and closing positions come from execution accounting.

```text
Committed close and positions at t
   |                         |
   v                         v
Background signals       Hedge targets
   |                         |
   +----------+--------------+
              v
      Common order limits
              |
              v
       Auction at t + 1
              |
              +----> Closing price, return and volatility
              |
              v
      Apply actual fills
              |
              v
       Closing positions
              |
              v
        Completed record
```

## Decisions at the opening step

Draws are addressed by market seed, opening step, optional agent ID, shock
kind and draw index. The default source maps each address to one uniform
innovation in `[-1, 1)`. The public fundamental and private background
signals occupy different shock addresses. A hedger's target and any resulting
orders leave those addresses unchanged.

The log fundamental receives a new public innovation at the opening step.
It starts at the log initial close. Each step adds 1% of the gap to that
initial value and 0.015 times the innovation. The fundamental supplies a
signal to fundamentalists. The auction supplies the traded close.

| Cohort | New signal |
| --- | --- |
| Fundamentalists | 0.60 × own previous signal + 0.40 × sensitivity × log fundamental gap + 0.008 × private innovation |
| Trend traders | 0.70 × own previous signal + 0.30 × sensitivity × latest realized log return + 0.008 × private innovation |
| Noise traders | 0.60 × own previous signal + 0.012 × private innovation |
| Hedgers | Absolute target in shares, supplied in hedger ID order |

The log fundamental gap is the log fundamental minus the log opening close.
Sensitivity is 1 + 0.05 × (agent ID modulo 11). A background signal's sign
sets its side. Its magnitude, scaled by 0.03 and capped at 1, sets a fraction
of the configured maximum order size. The result rounds to whole lots. The
limit price starts from the opening close and applies the signal as a log
price move bounded to ±0.05.

Hedge orders use the target minus actual holdings rounded toward zero in
lots. Their limits apply a log price move of +0.02 for buys and −0.02 for
sells. The same order and position caps apply to all agents. All limits
round to positive price ticks, and all orders enter the same batch auction.

## Completed day

Gross return is the closing price divided by the opening price. Let `r` be
the natural logarithm of gross return and `D` be the scenario's trading days
per year. The annualized variance uses this update.

```text
variance(t + 1) = 0.94 × variance(t) + 0.06 × D × r²
```

The next decision can use the square root of this variance after the day
commits. Step zero uses the configured volatility and a unit gross return.

The completed market record contains the new step, closing price, gross
return, annualized volatility, fills, executed volume and cohort positions
after execution. Each cohort reports net signed lots and the sum of absolute
lots. The caller supplies closing positions after applying fills. The day
checks each one against its opening position plus the signed lots from
actual fills before it produces the record.

These behavioral coefficients are initial scenario assumptions for integration.
Issue #11 owns calibration of the research configuration.
