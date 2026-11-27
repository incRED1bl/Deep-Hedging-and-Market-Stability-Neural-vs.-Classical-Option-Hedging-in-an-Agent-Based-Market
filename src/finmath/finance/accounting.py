"""Exact single-hedger cash reference using the issue #2 lot and cash units.

Observation -> actual fills -> funding on opening cash -> settlement -> issue.
Target positions and unfilled orders never enter this ledger.
AI assistance: ChatGPT; independent integer arithmetic and hand-calculated tests.
"""

from dataclasses import dataclass, replace
from decimal import ROUND_HALF_UP, Decimal

RATE_SCALE = 1_000_000_000
SHARE_SCALE = 1_000_000


def _integers(*values: int) -> None:
    if any(type(value) is not int for value in values):
        raise ValueError("cash, lots, prices and steps must be exact integers")


def _round_ratio(numerator: int, denominator: int) -> int:
    quotient, remainder = divmod(abs(numerator), denominator)
    return (quotient + (2 * remainder >= denominator)) * (-1 if numerator < 0 else 1)


def _rate_units(rate: float, *, nonnegative: bool = False) -> int:
    value = Decimal(str(rate))
    if not value.is_finite() or (nonnegative and value < 0):
        raise ValueError("invalid rate")
    return int((value * RATE_SCALE).to_integral_value(rounding=ROUND_HALF_UP))


def normalized_shortfall(pnl_units: int, issue_spot_units: int, size_shares: int) -> float:
    """Positive means a loss; use a complete contract's P&L and issuance notional."""
    _integers(pnl_units, issue_spot_units, size_shares)
    if issue_spot_units <= 0 or size_shares <= 0:
        raise ValueError("positive issuance notional required")
    return -pnl_units / (issue_spot_units * size_shares)


@dataclass(frozen=True)
class Instrument:
    """tick_value_per_share is in integer cash units; lot_size is in microshares."""

    cash_decimal_places: int
    tick_value_per_share: int
    lot_size_micros: int

    def __post_init__(self) -> None:
        _integers(self.cash_decimal_places, self.tick_value_per_share, self.lot_size_micros)
        if not 0 <= self.cash_decimal_places <= 9:
            raise ValueError("cash decimal places must be in [0,9]")
        if self.tick_value_per_share <= 0 or self.lot_size_micros <= 0:
            raise ValueError("positive tick and lot sizes required")

    def stock_value(self, price_ticks: int, signed_lots: int) -> int:
        _integers(price_ticks, signed_lots)
        if price_ticks <= 0:
            raise ValueError("positive stock price required")
        return _round_ratio(
            price_ticks * self.tick_value_per_share * signed_lots * self.lot_size_micros,
            SHARE_SCALE,
        )

    def fee(self, notional_units: int, commission: float) -> int:
        _integers(notional_units)
        if notional_units < 0:
            raise ValueError("nonnegative trade notional required")
        return _round_ratio(notional_units * _rate_units(commission, nonnegative=True), RATE_SCALE)

    def funding(self, opening_cash_units: int, annual_rate: float, days_per_year: int) -> int:
        _integers(opening_cash_units, days_per_year)
        if days_per_year <= 0:
            raise ValueError("positive day count required")
        return _round_ratio(
            opening_cash_units * _rate_units(annual_rate), RATE_SCALE * days_per_year
        )

    def price_units(self, price_per_share: float) -> int:
        """Convert a nonnegative model price to the instrument's booked cash precision."""
        value = Decimal(str(price_per_share))
        if not value.is_finite() or value < 0:
            raise ValueError("nonnegative finite model price required")
        return int((value * 10**self.cash_decimal_places).to_integral_value(rounding=ROUND_HALF_UP))


@dataclass(frozen=True)
class Fill:
    """A signed executed quantity: positive for a buy, negative for a sell."""

    signed_lots: int
    price_ticks: int

    def __post_init__(self) -> None:
        _integers(self.signed_lots, self.price_ticks)
        if self.signed_lots == 0 or self.price_ticks <= 0:
            raise ValueError("nonzero executed quantity and positive price required")


@dataclass(frozen=True)
class ShortCall:
    size_shares: int
    strike_ticks: int
    premium_per_share_units: int
    issued_at: int
    expires_at: int

    def __post_init__(self) -> None:
        _integers(
            self.size_shares,
            self.strike_ticks,
            self.premium_per_share_units,
            self.issued_at,
            self.expires_at,
        )
        if self.size_shares <= 0 or self.strike_ticks <= 0 or self.premium_per_share_units < 0:
            raise ValueError("invalid call size, strike or premium")
        if self.issued_at < 0 or self.expires_at <= self.issued_at:
            raise ValueError("expiry must follow issuance")


@dataclass(frozen=True)
class HedgeAccount:
    """Immutable short-call hedger: negative cash is retained, without injections.

    This is a financial oracle, not an order executor or a margin model. Python
    integers retain exact intermediate values instead of silently overflowing.
    """

    instrument: Instrument
    cash_units: int
    net_lots: int = 0
    step: int = 0
    call: ShortCall | None = None

    def __post_init__(self) -> None:
        _integers(self.cash_units, self.net_lots, self.step)
        if self.step < 0 or (
            self.call is not None and not self.call.issued_at <= self.step < self.call.expires_at
        ):
            raise ValueError("invalid account step or expired active call")

    def issue(self, call: ShortCall) -> "HedgeAccount":
        if self.call is not None or call.issued_at != self.step:
            raise ValueError("issue requires an empty call slot at the current close")
        return replace(
            self,
            cash_units=self.cash_units + call.premium_per_share_units * call.size_shares,
            call=call,
        )

    def advance_day(
        self,
        close_ticks: int,
        fills: tuple[Fill, ...] = (),
        *,
        commission: float = 0.0,
        annual_funding_rate: float = 0.0,
        days_per_year: int = 252,
    ) -> "HedgeAccount":
        _integers(close_ticks)
        if close_ticks <= 0:
            raise ValueError("positive closing price required")
        # Validate even an empty day, before computing any new state.
        _rate_units(commission, nonnegative=True)
        cash, lots = self.cash_units, self.net_lots
        for fill in fills:
            notional = self.instrument.stock_value(fill.price_ticks, abs(fill.signed_lots))
            direction = 1 if fill.signed_lots > 0 else -1
            cash -= direction * notional + self.instrument.fee(notional, commission)
            lots += fill.signed_lots
        cash += self.instrument.funding(self.cash_units, annual_funding_rate, days_per_year)
        next_step = self.step + 1
        call = self.call
        if call is not None:
            if call.expires_at < next_step:
                raise ValueError("overdue call")
            if call.expires_at == next_step:
                cash -= (
                    max(close_ticks - call.strike_ticks, 0)
                    * self.instrument.tick_value_per_share
                    * call.size_shares
                )
                call = None
        return replace(self, cash_units=cash, net_lots=lots, step=next_step, call=call)

    def equity(self, close_ticks: int, *, call_mark_per_share_units: int | None = None) -> int:
        """Marked equity; an outstanding call needs an explicit common model mark."""
        equity = self.cash_units + self.instrument.stock_value(close_ticks, self.net_lots)
        if self.call is not None:
            if call_mark_per_share_units is not None:
                _integers(call_mark_per_share_units)
            if call_mark_per_share_units is None or call_mark_per_share_units < 0:
                raise ValueError("nonnegative explicit mark required for a live call")
            equity -= call_mark_per_share_units * self.call.size_shares
        elif call_mark_per_share_units is not None:
            raise ValueError("cannot mark an inactive call")
        return equity

    def pnl(
        self,
        opening_equity_units: int,
        close_ticks: int,
        *,
        call_mark_per_share_units: int | None = None,
    ) -> int:
        _integers(opening_equity_units)
        return (
            self.equity(close_ticks, call_mark_per_share_units=call_mark_per_share_units)
            - opening_equity_units
        )
