# Deep Hedging and Market Stability

Neural vs. Classical Option Hedging in an Agent-Based Market.

Сравниваем нейросетевое и классическое хеджирование опционов в агентном рынке:
качество хеджа, издержки и влияние стратегий на устойчивость рынка.

## Разработка

Нужны Python 3.12 и **uv 0.13.9**. Точная версия Python указана в
`.python-version`, зависимости — в `uv.lock`.

```bash
uv python install
uv sync --locked
uv run ruff format --check .
uv run ruff check .
uv run pytest
```

В Python доступны финансовые эталоны и классические стратегии хеджирования;
Rust содержит рынок, общий интерфейс политик и классические стратегии.
Интеграция полной бухгалтерии, обучение и исследовательские прогоны ведутся
в задачах backlog. GPU и Kaggle не нужны для обычного CI.

| Участник | GitHub | Область |
| --- | --- | --- |
| Тим | [@incRED1bl](https://github.com/incRED1bl) | Deep hedging, обучение, выбор и фиксация модели |
| Даня | [@bani2016up](https://github.com/bani2016up) | Рынок, агенты, исполнение заявок, портфели |
| Федя | [@Perf0rator4](https://github.com/Perf0rator4) | Классические стратегии, финансовая математика, статистика |
| Лев | [@Levonidor](https://github.com/Levonidor) | Данные, калибровка, наборы для Kaggle, итоговые запуски |

## Рабочие документы

- [Задачи и DoD](https://github.com/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market/issues)
- [Правила разработки и review](CONTRIBUTING.md)
- [Данные и обучение на Kaggle](docs/data-and-kaggle.md)
- [Финансовый эталон: Black–Scholes, греки и P&L](docs/finance-reference.md)
- [Классические стратегии: формулы, литература и контрольные сценарии](docs/classical-strategies.md)
- [Настройки защиты репозитория](docs/repository-settings.md)

В `main` изменения попадают через PR после review и успешного CI. Датасеты,
веса и credentials в Git не хранятся. Исследовательский протокол фиксируется
в задаче #1 до итоговых экспериментов.
