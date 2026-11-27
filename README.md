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

Пока добавлена только основа Python-пакета и проверка его установки. Симулятор,
классические стратегии, обучение и исследовательские тесты реализуются в задачах
backlog. GPU и Kaggle не нужны для обычного CI.

| Участник | Область |
| --- | --- |
| Тим | Deep hedging, обучение, выбор и фиксация модели |
| Даня | Рынок, агенты, исполнение заявок, портфели |
| Федя | Классические стратегии, финансовая математика, статистика |
| Лев | Данные, калибровка, наборы для Kaggle, итоговые запуски |

## Рабочие документы

- [Задачи и DoD](https://github.com/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market/issues)
- [Правила разработки и review](CONTRIBUTING.md)
- [Данные и обучение на Kaggle](docs/data-and-kaggle.md)
- [Настройки защиты репозитория](docs/repository-settings.md)

В `main` изменения попадают через PR после review и успешного CI. Датасеты,
веса и credentials в Git не хранятся. Исследовательский протокол фиксируется
в задаче #1 до итоговых экспериментов.
