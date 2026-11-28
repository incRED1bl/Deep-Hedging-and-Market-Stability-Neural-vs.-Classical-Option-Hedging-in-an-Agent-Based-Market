# Метрики запуска и статистический анализ (issue #13)

Документ фиксирует схему строки метрик одного запуска и статистический пайплайн
для проверки трёх гипотез исследовательского протокола (#1, `docs/protocol.md`).
Реализация: `src/finmath/metrics/` и `src/finmath/statistics/` (только стандартная
библиотека Python 3.12, без дополнительных runtime-зависимостей).

## 1. Схема строки одного запуска (`RunMetricsRow`)

Один запуск идентифицируется ключом `(policy, cost_bps, seed)`, совместимым
с реестром экспериментов #15 (`policy ∈ {"bs", "leland", "band", "dl"}`,
`cost_bps ∈ {0, 5, 10, 20, 40}`, `seed` — точное целое `uint64`, в JSON
сериализуется десятичной строкой `str(seed)`).

### Исключение burn-in

- Опорный шаг `step = burn_in_days` (в протоколе `B = 500`) задаёт только
  начальную цену `P_B` и начальный капитал хеджеров `equity_B`.
- Все observed-метрики считаются строго по `M = observed_days` шагам
  `step = B + 1, ..., B + M` (`501..2500`).
- Дни без сделок (`bilateral_executed_shares == 0`) сохраняют цену `P_t = P_{t-1}`,
  дают `r_t = 0` и **остаются** в ряду доходностей и оборота.

### Основные и диагностические поля

| Поле | Определение и единицы |
| --- | --- |
| `annualized_realized_volatility` (`V`) | `sqrt(D) * sample_sd(log(P_t / P_{t-1}), ddof=1)` по `M` доходностям (`t = B+1..B+M`), доля в годовом выражении |
| `executed_shares_per_day` (`U`) | `sum(Q_t) / M`, акции/день; каждая bilateral сделка считается **один раз** |
| `hedger_executed_shares_per_day` (`U_hedge`) | Среднее за день `sum_h \|actual fills_h\|` по хеджерам (не складывается с `U`) |
| `dollar_volume_per_day` | Средний дневной стоимостной оборот (переданный или `Q_t * P_t`) |
| `total_commission_paid` | Суммарные комиссии хеджеров за observed-окно (burn-in исключён) |
| `net_pnl_observed`, `net_pnl_per_hedger` | `equity_{B+M} - equity_B` по когорте хеджеров и среднее на хеджера (комиссии уже внутри cash/equity, повторно не вычитаются) |
| `lifetime_net_pnl` | `equity_{B+M} - equity_0`, если передан шаг `0`, иначе `null` |
| `max_drawdown` | `max_t (1 - P_t / max_{u <= t} P_u)` на `[B, B + M]` |
| `no_trade_day_share` | Доля observed-дней с `Q_t == 0` |
| `unfilled_hedge_demand_share` | `sum(unfilled) / sum(requested)` или `null`, если спрос равен `0` |
| `limit_hit_count` | Число попаданий в лимиты за observed-окно |
| `negative_equity_days`, `min_hedger_equity` | Диагностика отрицательного equity (убытки сохраняются, seed не исключается) |
| `mean_open_calls_shares` | Средний объём открытых коротких call за observed-окно |
| `zero_booked_fee_share` | Доля fills с нулевой booked fee среди fills при `cost_bps > 0`, иначе `null` |

### Контрактные hedging errors и неполные контракты

- Полным для observed-окна считается контракт с `settled=True`,
  `issue_step >= burn_in_days` и `expiry_step <= burn_in_days + observed_days`.
- Неполные контракты (выпущенные до `B`, погашенные после `B + M` или
  непогашенные) **исключаются** из контрактных ошибок; их количество сохраняется
  в `incomplete_contracts_excluded`, а число полных — в `complete_contracts_count` (`K`).
- Нормировка и знак: `z_k = net_pnl_k / (size_shares_k * issue_spot_k)`,
  `epsilon_k = -z_k` (положительная ошибка означает shortfall).
- Внутри запуска (одинаковый вес каждого полного контракта):
  - `hedging_error_mean = mean(epsilon_k)`
  - `hedging_error_rmse = sqrt(mean(epsilon_k^2))`
  - `hedging_error_tail_loss` — среднее `ceil(0.05 * K)` наибольших `epsilon_k`
- При `K == 0` все три поля равны `null` (`NA`), **не нулю**.

## 2. Статистический анализ (`finmath.statistics`)

1. **Единица анализа — market seed.** Матрица проверяется на уникальность ключей
   и полноту всех парных ячеек; молчаливая complete-case фильтрация запрещена.
2. **Три подтверждающие гипотезы (`alpha = 0.05`, односторонние `less`):**
   - `H1`: парный t-test по `D_V(s, c0) = V(s, dl, c0) - V(s, bs, c0)`, `df = N - 1`.
   - `H2`: парный t-test по `D_U(s, c0) = U(s, dl, c0) - U(s, bs, c0)`, `df = N - 1`.
   - `H3`: внутрисеменной OLS slope `b_s` разности `D_V(s, c_j)` по
     `x_j = c_j / c0 = [0, 0.5, 1, 2, 4]`, затем одновыборочный t-test по `b_s`
     (`df = N - 1`, без наивных pooled OLS SE).
3. **Поправка Holm (`holm_adjust`):** по трём гипотезам `H1/H2/H3`; сохраняются
   `p_raw`, `p_adjusted`, `statistic`, `df`, `n_seeds`, естественный размер эффекта,
   относительное изменение (`null` при нулевом среднем BS) и `d_z = mean / sd`.
4. **Вырожденные случаи (`sd == 0`):**
   - `mean == 0`: `statistic = 0`, `p_raw = 1`, `t_ci = None` (`degenerate=True`).
   - `mean != 0`: искусственное `p = 0` **не присваивается**; `statistic = None`,
     `estimable = False`, `p_raw = 1` (консервативно для Holm), `rejected = False`,
     `t_ci = None`.
   - Точечный percentile bootstrap при нулевой дисперсии помечается `degenerate=True`
     без заявления о 95% покрытии.
5. **Доверительные интервалы с явным типом покрытия:**
   - Маргинальный двухсторонний 95% t-интервал (`coverage_type="marginal"`).
   - Семейный Bonferroni t-интервал уровня `1 - alpha / 3` (`coverage_type="familywise-bonferroni"`).
   - Маргинальный 95% **whole-market-seed-block percentile bootstrap**
     (`resamples = 20_000`, `seed = 71001`): в каждом повторе пересэмплируются
     индексы целых seed-блоков (не дни и не агенты), одновременно для всех
     гипотез, уровней комиссий и компараторов (`bs`, `leland`, `band`).
6. **Диагностика распределений (`distribution_diagnostics`):**
   квантили, `IQR`, выборочные `skewness` и `excess_kurtosis`, крайние seeds,
   выбросы по `1.5 * IQR`, leave-one-seed-out оценки и флаг смены знака,
   нормальные QQ-точки (Blom) и гистограмма, плюс флаг расхождения `t_ci` и
   `bootstrap_ci` по покрытию нуля.
