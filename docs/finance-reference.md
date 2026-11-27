# Финансовый эталон: issue #5

Независимая реализация в `src/finmath/finance/`, без новых runtime dependencies.
Источник модели: [Black & Scholes (1973)](https://doi.org/10.1086/260062).
Код и примеры подготовлены с помощью ChatGPT и проверены аналитическими
пределами, ручными денежными потоками и численными производными.

## Цена, единицы и греки

`european_call(spot, strike, years, volatility, rate=0)` — European call без
дивидендов. spot/strike в одной валюте, years — торговые дни / 252,
volatility — годовая sigma в долях. rate — **непрерывная** годовая ставка.
Цена и греки возвращаются на одну акцию базового актива:

- delta = dC/dS; short-call хеджер держит `size_shares * delta` акций;
- gamma = d²C/dS²;
- vega = dC/dsigma, на единицу sigma; для движения на 1 процентный пункт
  умножить на 0.01.

При T=0 цена равна payoff. На детерминированном kink принята delta=0.5,
gamma=None, поскольку обычной второй производной там нет. При sigma=0
возвращается дисконтированный intrinsic; vega на ATM — правая производная.
Искусственного sigma floor нет. Неопределённая gamma не подменяется нулём.
При некорректных/непредставимых float inputs возвращается ValueError.
Это скалярный численный эталон, не дифференцируемый PyTorch training layer.

## Ставка: устранение разницы между pricing и ledger

Соглашение #2: cash accrues по простой дневной ставке `r_simple / D`.
Чтобы pricing использовал тот же денежный счёт до округления, выбираем:

```text
r_cont = D * log(1 + r_simple / D)
exp(-r_cont * days / D) = (1 + r_simple / D) ** (-days)
```

Использовать `funding_to_continuous_rate(r_simple, D)`. Входная ставка
квантуется до 1e-9, как FundingRate Rust. Дневной множитель должен быть
положительным. При r=0 оба соглашения совпадают; базовый протокол не меняется.
При r_simple=0.25, D=1, S=120, K=100, T=1, sigma=0 цена **40**, а не
42.1199, получаемая при трактовке тех же 0.25 как непрерывной ставки.
При S=79 дельта в согласованной детерминированной модели равна 0.

PR #29 использовал непрерывное pricing с простой cash rate; выбранное здесь
преобразование предлагается как единая финансовая конвенция для #8/#9/#10/#14.
Его нужно принять на review и применить в Rust pricing adapter; этот PR не
включает изменения ещё не влитой реализации #29. Округление дневных процентов
до cash unit остаётся дискретной погрешностью, не отменяющей общей конвенции.

## Фактические сделки и P&L

`Instrument` фиксирует integer cash precision, tick value в cash units и lot
size в microshares (1 share = 1 000 000). Суммы и количества — целые числа.
Rates квантуются до billionths; cash округляется к ближайшему unit,
половины — от нуля. Сначала округляется notional, потом fee.
`price_units` переводит неотрицательную модельную цену в booked cash units.

`HedgeAccount` — неизменяемое состояние одного продавца call. `Fill` содержит
фактическое signed количество lots, не target и не размер заявки.
`advance_day` выполняет:

1. Fills: `cash -= signed_notional + one_way_fee`; меняются только исполненные lots.
2. Funding на cash в начале интервала, до fills текущего дня.
3. Cash settlement call по новой цене закрытия, если наступила экспирация.
4. Возвращает новое состояние. Новый call можно выпустить `issue` в этом close.

Старая stock позиция и cash сохраняются при roll. Call погашается один раз.
Отрицательный cash не обнуляется и не исключается. Ошибка не изменяет исходный
account; повторная попытка начинается с того же состояния. Это кредитное
допущение, а не модель дефолта и принудительной ликвидации.

```text
equity_t = cash_t + stock_mark_t - live_call_mark_t * size
net_P&L = equity_end - equity_start
```

Комиссии, premium, funding и payoff уже в cash и повторно не вычитаются.
При живом call требуется явный per-share mark; без него ошибка, не нулевая
стоимость обязательства. Все политики используют общую BS mark-функцию с
наблюдаемой sigma, не собственный Leland/DL pricing. Для observed P&L
baseline — equity после close 500, с его живыми call, а не исходный депозит.

Нормированная signed ошибка полного контракта:
`epsilon = -net_P&L / (issue_spot * size_shares)` после перевода P&L из cash
units в валюту. Положительная ошибка — shortfall.
Функция `normalized_shortfall(pnl_units, issue_spot_units, size_shares)` считает
тот же показатель непосредственно в одинаковых cash units.
Оставшийся stock оценивается
по expiry close без выдуманной принудительной сделки. При разнесённых roll
контрактный subledger и передача stock между ним и новым контрактом относятся
к #8/#13; агрегатный hedge P&L нельзя объявлять ошибкой одного контракта.

## Ручной пример и общий fixture

Начало: $200 cash + 2 акции по $100, equity=$400. Продан call на 2 акции,
K=$100, premium=$2.50/акция. Из заявки купить 3 акции исполнена **1** по $100;
fee=$0.10. Funding на $205 при r_simple=0.252,D=252 равен $0.21.

| Expiry close | Payoff | Итоговый cash | Stock | Net P&L |
| --- | ---: | ---: | ---: | ---: |
| $110 | $20 | $85.11 | $330 | $15.11 |
| $100 | $0 | $105.11 | $300 | $5.11 |
| $90 | $0 | $105.11 | $270 | -$24.89 |

Числа записаны в [общий JSON fixture](../tests/fixtures/finance-reference.json).
Python проверяет его через независимый account; Rust integration test использует
его с настоящим аукционом, частичным fill и domain cash/funding/payoff функциями.
Это ещё не приёмка полного Ledger из draft #28 или trained replay из #9.

Старый cents fixture обнуляет fee для $2 notional при 10 bps. Новый fixture
ветки #29 (`de3a66a`) имеет cash precision=8 и tick value=1 000 000 units;
экономическая цена остаётся $100. Python regression проверяет оба масштаба,
не вводя минимальную комиссию и не увеличивая искусственно номинал позиции.

## Использование и проверки

```python
from finmath.finance import Instrument, european_call, funding_to_continuous_rate

instrument = Instrument(8, 1_000_000, 10_000)
rate = funding_to_continuous_rate(0.05, 252)
call = european_call(100, 100, 20 / 252, 0.20, rate)
booked_mark = instrument.price_units(call.price)
```

```sh
uv run pytest tests/finance
cargo test --locked --test finance_reference
```

При подготовке также проверен реальный Ledger ветки #28, commit `326a34c`:
в отдельном временном checkout он получил те же три JSON-сценария и реальные
fills аукциона. Cash и P&L совпали с независимым эталоном во всех трёх случаях.
Это проверка этих контрольных сценариев, а не полного пространства состояний
или интеграции с ещё не созданной обучающей средой.

Эти проверки устанавливают соответствие формул и денежных конвенций.
Исследование рыночных эффектов и оценка DL выполняются в последующих задачах.
