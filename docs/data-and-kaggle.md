# Данные и обучение на Kaggle

**Лев готовит и публикует private synthetic dataset в своём Kaggle-аккаунте,
даёт Тиму доступ на чтение; Тим обучает модель в своём private kernel.**
Датасеты и kernel ещё не созданы. В `kaggle/templates/` нужно заменить все
`REPLACE_WITH_...` и `null` перед использованием.

## Ответственность и разделение данных

| Артефакт | Кто готовит | Где используется |
| --- | --- | --- |
| Исходные рыночные данные, журнал очистки и калибровка | Лев; параметры проверяет Федя | Локальная подготовка данных |
| Замороженный пакет train/validation и его публикация | Лев | Private dataset Льва; у Тима `Can view` |
| Обучение, выбор checkpoint и конфигурация модели | Тим | Private Kaggle kernel только с train/validation |
| Финальные seeds и конфигурации эксперимента | Лев и Федя | Отдельный закрытый пакет для финальной оценки |
| Сверка версии кода, весов и результатов | Даня и Федя | Воспроизводимый финальный запуск |

Рыночную историю делим **хронологически** до подбора параметров. Калибровка
симулятора и нормализация признаков используют только разрешённую обучающую
часть. Synthetic train и validation получают непересекающиеся списки seeds;
источники случайности рынка и инициализации сети хранятся раздельно.

Final-test хранится отдельно у Льва/Феди: он не входит в training dataset,
не подключается к training kernel и не влияет на выбор модели. До freeze
Тим получает только идентификатор протокола и SHA256 пакета. После фиксации
commit кода, hash весов, правила выбора checkpoint и параметров эксперимента
Федя открывает пакет для оценки. До этого final-test не расшаривается Тиму:
private-статус не скрывает данные от владельца или приглашённого пользователя.

## Что передаёт Лев

Пакет размещается локально в `data/kaggle-upload/<release-id>/`; большие данные
и credentials не коммитятся. Для простого upload файлы лежат в корне пакета.

| Файл | Контракт |
| --- | --- |
| `dataset-metadata.json` | Kaggle title, `owner/slug`, описание и применимая лицензия; шаблон рядом |
| `manifest.json` | Версия схемы, release ID, commit генератора, hash `uv.lock`, режим генерации, список файлов с SHA256 и размерами |
| `calibration.json` | Инструмент, источник, период, параметры и их единицы; для каждого параметра — оценён/из литературы/допущение |
| `simulation-config.json` | Параметры генератора, шаг времени, burn-in, число агентов, схема исполнения и контракты опционов |
| `train-seeds.json` | JSON-массив уникальных десятичных строк с seeds обучающих эпизодов |
| `validation-seeds.json` | Другой JSON-массив seeds; пересечение с train пустое |
| `DATA_CARD.md` | Происхождение, очистка, хронологический split, ограничения и разрешённое использование |
| `train-paths.npz`, `validation-paths.npz` | Только если выбрано предварительное обучение на фиксированных синтетических траекториях |

В `manifest.files` перечисляются **все payload-файлы**; сам manifest и Kaggle
metadata туда не входят. Hash manifest фиксируется снаружи пакета в
`artifact-pin.json` и конфигурации запуска. Неполные списки файлов и
placeholders недопустимы в готовом релизе.

Для `.npz` контракт — `spot: float64[N, H+1]`,
`vol_observed: float64[N, H+1]`, `episode_seed: uint64[N]`,
`strike: float64[N]`; `H`, `dt_years`, ставка, дивидендная конвенция,
notional и комиссия задаются в config. Все цены и страйки положительны;
массивы конечны, порядок эпизодов согласован с seeds. Загрузка —
`numpy.load(..., allow_pickle=False)`. `vol_observed` вычисляется только из
доступной агенту истории; будущие значения и скрытая истинная волатильность
не становятся признаками. Текущая позиция зависит от действий политики и
вычисляется во время обучения, а не подставляется из чужой стратегии.

Seed эпизода находится в диапазоне Rust `u64` от `0` до
`18446744073709551615` включительно. В обоих JSON-списках он записывается
строкой из цифр без знака и ведущих нулей, например `"9223372036854775808"`
для `2**63`. В Python декодируем строки в точный `int` функцией
`finmath.data_release.read_seeds`, затем строим `np.array(seeds, dtype=np.uint64)`.
Порядок `episode_seed` в NPZ должен совпадать с соответствующим JSON-списком.
При чтении NPZ возвращаем семя в Python через `int(seed)`. Все шаги проходят
без `float`. При передаче конкретного seed в `simulation-config.json` Rust
декодирует целочисленное поле `seed` типом `u64`. Записываем его из точного
целого и избегаем JSON-инструментов, переводящих числа в `float`.

`spot` и `strike` в NPZ измеряются в валюте котировки за одну акцию. Rust
хранит цены в целых тиках. В `simulation-config.json` размер тика в этих
единицах равен `tick_size = instrument.tick_value_per_share * 10**(-instrument.cash_decimal_places)`.
Экспортёр вычисляет `spot = spot_ticks * tick_size` и
`strike = strike_ticks * tick_size`, затем записывает результаты в `float64`.
При `tick_value_per_share=1` и `cash_decimal_places=2` размер тика
равен `0.01`, поэтому `10_000` тиков дают цену `100.00` за акцию.

Фиксированные price paths подходят для предварительного deep hedging.
**Они не заменяют финальный интерактивный рынок:** для market-stability
эксперимента симулятор запускается заново с зафиксированными seeds, и заявки
хеджеров участвуют в формировании цены. Поле `generation_mode` различает
`exogenous_pretrain` и `interactive_seeded`. Во втором режиме пакет содержит
seeds/config, а траектории возникают при запуске политики.

### Рыночная история и права на данные

Исходные котировки и собственные синтетические эпизоды — разные артефакты.
Их нельзя объединять под общей придуманной лицензией. Для FRED `SP500`
официальная страница указывает price index без дивидендов, десять лет дневной
истории и ограничения S&P на воспроизведение. Поэтому raw FRED/SP500 не
включаем автоматически в Kaggle upload или публичный Git: в репозитории
остаётся код получения, ссылка, период, hash локальной копии и описание
калибровки. Private upload тоже не является разрешением на перераспространение.
Синтетические пути должны генерироваться нашей моделью, а не быть копией
исходных рядов с переименованными колонками. Для них отдельно описывается
происхождение. [Источник: FRED SP500](https://fred.stlouisfed.org/series/SP500).

В metadata выбран `copyright-authors` для собственного synthetic-пакета;
перед загрузкой команда проверяет, что он действительно применим ко всем
его файлам. Это не лицензия на исходные биржевые данные.

## Локальный CLI через uv

Команды выполняются из корня репозитория. Версия пакета `kaggle` зафиксирована
в `uv.lock`; группа ставится явно:

```bash
uv sync --locked --group kaggle
uv run --group kaggle kaggle --version
uv run --group kaggle kaggle auth login
```

Лев и Тим авторизуются **каждый в своём аккаунте** и берут usernames из
профилей Kaggle. CLI поддерживает OAuth и
`KAGGLE_API_TOKEN`; credentials не передаются друг другу и не записываются в
Git, metadata, notebook или логи. В обычном CI Kaggle auth и upload не нужны.
[Официальная авторизация](https://github.com/Kaggle/kaggle-cli/blob/main/docs/README.md#authentication).

### Лев: создать private training dataset

Упаковщик собирает точный набор payload-файлов, проверяет принадлежность seed
к train/validation по протоколу и записывает размеры и SHA256 в manifest.
На вход подаётся отдельная папка с синтетическими payload-файлами. В режиме
`exogenous_pretrain` сюда также входят NPZ, заранее сверенные с контрактом #9.
Сборка проверяет целостность NPZ по хешам. Численные свойства и причинность
признаков проверяет генератор при подготовке входов.

```bash
uv run python -m finmath.kaggle_package \
  "$FINMATH_SYNTHETIC_PAYLOAD" "$FINMATH_RELEASE_DIR" \
  --dataset "$FINMATH_DATASET" --title "$FINMATH_TITLE" \
  --mode "$FINMATH_GENERATION_MODE" --created-at "$FINMATH_RELEASE_CREATED_AT" \
  --commit "$FINMATH_GENERATOR_COMMIT" --protocol research/protocol-v1.json \
  > "$FINMATH_LOCAL_PIN"
```

Путь `FINMATH_LOCAL_PIN` указывает на файл вне каталога загрузки. После
публикации добавьте в pin фактические `dataset_version` и `local_path` по
шаблону `kaggle/templates/artifact-pin.json.example`. Перед загрузкой сверьте
численные входы и применимость лицензии ко всем payload-файлам.

```bash
FINMATH_RELEASE_DIR="data/kaggle-upload/REPLACE_WITH_RELEASE_ID"
FINMATH_DATASET="REPLACE_WITH_DATASET_OWNER/REPLACE_WITH_DATASET_SLUG"
uv run --group kaggle kaggle datasets create -p "$FINMATH_RELEASE_DIR" -t
uv run --group kaggle kaggle datasets status "$FINMATH_DATASET"
```

По документации dataset создаётся private, если не передавать `--public`;
`-t` сохраняет исходный формат табличных файлов. Папка для загрузки должна
содержать только подготовленный release, а не весь репозиторий.

Для исправления **чернового, ещё не замороженного** пакета:

```bash
uv run --group kaggle kaggle datasets version -p "$FINMATH_RELEASE_DIR" -t -m "REPLACE_WITH_EXACT_CHANGE_NOTES"
```

После freeze каждый изменённый пакет получает новый release ID и новый slug.
Старые релизы сохраняются. Ни `--delete-old-versions`, ни удаление datasets
не входят в этот процесс. [Официальные команды datasets](https://github.com/Kaggle/kaggle-cli/blob/main/docs/datasets.md).

### Лев: дать Тиму доступ; Тим: проверить релиз

На странице dataset Лев открывает **Settings → Sharing**, находит точный
Kaggle username Тима и добавляет его с правом **Can view**. Dataset остаётся
private; доступ к kernel настраивается отдельно. Это поддерживаемый Kaggle
сценарий: [Dataset collaboration](https://www.kaggle.com/docs/datasets).

После успешной загрузки заполнить `artifact-pin.json` по шаблону: реальный
`owner/slug`, **номер версии, возвращённый Kaggle**, SHA256 manifest, commit
генератора и локальный путь. Лев передаёт Тиму ссылку и этот pin.
**Тим под своей авторизацией** проверяет доступ и скачивает релиз. Замените
путь `pin` на путь к полученному `artifact-pin.json` вне папки скачивания.

```bash
(
  set -e
  pin="REPLACE_WITH_PATH_TO_ARTIFACT_PIN_JSON"
  dataset=$(jq -er '.dataset | select(test("^[A-Za-z0-9_-]+/[a-z0-9-]+$"))' "$pin")
  version=$(jq -er '.dataset_version | select(type == "number" and . > 0 and floor == .)' "$pin")
  release=$(jq -er '.release_id | select(test("^[A-Za-z0-9][A-Za-z0-9._-]*$"))' "$pin")
  directory="data/kaggle-download/$release/v$version"
  mkdir -p "$(dirname "$directory")"
  mkdir "$directory"
  uv run --locked --group kaggle kaggle datasets download "$dataset/$version" -p "$directory" --unzip
  uv run --no-sync python -m finmath.data_release "$directory" "$pin"
)
```

`mkdir` останавливает повторное скачивание в уже существующую папку этой
версии. Если скачивание прервалось, отложите незавершённую папку перед
повтором. Используем релиз после успешной проверки SHA256 manifest по внешнему
pin, точного состава каталога, размеров, хешей и непересекающихся seeds.
Скачанный пакет содержит `manifest.json` и файлы из `manifest.files`.
`dataset-metadata.json` используется CLI при загрузке и в пакет не попадает.

## Тим: private training kernel

`kaggle/templates/kernel-metadata.json.example` задаёт private Python script
с GPU, без доступа в интернет и только с training dataset. Тим копирует его
как `kaggle/local-kernel/kernel-metadata.json`: в `id` указывает **свой**
username, в `dataset_sources` — **username Льва** и release slug. Также нужен
реальный `train.py`; обучающий код в этот foundation не входит.

Kaggle позволяет подключать в notebook datasets, к которым пользователь имеет
доступ: [Adding Data Sources](https://www.kaggle.com/docs/notebooks).
CLI передаёт их через `dataset_sources`; доступ для конкретной пары аккаунтов
проверяется при передаче, а не считается уже настроенным. Если CLI не
подключает доступный dataset, Тим проверяет его через **Input → Add Input**
по ссылке. Только если private collaboration недоступна, резервный путь —
Лев передаёт готовый пакет, Тим загружает копию в свой аккаунт и обновляет
dataset owner/version в pin, сохраняя прежние hashes payload. Credentials
при этом тоже не передаются.

Официальная схема `dataset_sources` документирует `owner/slug`, без фиксации
версии в этом поле. Поэтому template не обещает автоматический version pin:
используем отдельный замороженный release slug, а в `train.py` фиксируем
ожидаемый SHA256 manifest из внешнего `artifact-pin.json` и проверяем его
**до первого шага оптимизатора**. Иной подключённый release должен привести
к ошибке. [Схема kernel metadata](https://github.com/Kaggle/kaggle-cli/blob/main/docs/kernels_metadata.md).

Подготовленный код можно сохранить на Kaggle без выполнения:

```bash
uv run --group kaggle kaggle kernels push -p kaggle/local-kernel --no-run
```

`kernels push` **без** `--no-run` запускает kernel. На этапе настройки
репозитория этого не делаем. После отдельно выбранного запуска сохраняем
реальный номер версии kernel, и результаты получаем именно из него:

```bash
FINMATH_KERNEL="REPLACE_WITH_TIM_KAGGLE_USERNAME/REPLACE_WITH_KERNEL_SLUG"
FINMATH_KERNEL_VERSION="REPLACE_WITH_ACTUAL_KERNEL_VERSION"
uv run --group kaggle kaggle kernels status "$FINMATH_KERNEL/$FINMATH_KERNEL_VERSION"
uv run --group kaggle kaggle kernels output "$FINMATH_KERNEL/$FINMATH_KERNEL_VERSION" -p artifacts/kaggle-output
```

[Официальные команды kernels](https://github.com/Kaggle/kaggle-cli/blob/main/docs/kernels.md).
Локальный `uv.lock` не фиксирует предустановленный образ Kaggle. Перед
обучением Тим проверяет совместимость и сохраняет фактические версии
Python, PyTorch, CUDA и зависимостей. При `enable_internet=false` нужный код
и отсутствующие зависимости должны быть заранее подготовлены для offline
установки; сам metadata template этого не делает.

### Что должно выйти из обучения

Весам сопутствуют `run.json`, конфигурация модели, hash весов, версия kernel,
pin входного dataset, commit кода, seed инициализации сети, версии runtime,
train/validation-метрики и правило выбора checkpoint. Дополнительно нужны
необходимые для inference параметры нормализации и их происхождение.
Финальные тестовые метрики не участвуют в выборе checkpoint.

## Критерий готовности передачи

- Лев воспроизводит пакет из указанного commit, calibration и seeds.
- В готовом manifest нет placeholders; каждый payload-файл учтён.
- Лев публикует private dataset; Тим под своим аккаунтом читает его и
  подключает к своему private kernel с известными version и SHA256.
- Training kernel не имеет final-test inputs и проверяет входы до обучения.
- Другой участник скачивает указанную версию и подтверждает те же hashes.
- Федя хранит отдельно зафиксированный final-test пакет до freeze модели.

## Источники и происхождение шаблонов

CLI-синтаксис и metadata сверены с официальным `Kaggle/kaggle-cli` 27.11.2026.
Локально проверены `--help` этих команд в Kaggle CLI 2.2.5 и разбор версии
dataset установленным клиентом; авторизация, загрузка и запуск не выполнялись.
Источники:
[datasets](https://github.com/Kaggle/kaggle-cli/blob/main/docs/datasets.md),
[dataset metadata](https://github.com/Kaggle/kaggle-cli/blob/main/docs/datasets_metadata.md),
[kernels](https://github.com/Kaggle/kaggle-cli/blob/main/docs/kernels.md),
[kernel metadata](https://github.com/Kaggle/kaggle-cli/blob/main/docs/kernels_metadata.md).
Схема нашего manifest и протокол разделения данных — решения проекта, а не
поля Kaggle API. Документ и `kaggle/templates/` подготовлены с помощью
ChatGPT; перед использованием участники проверяют значения и применимость
контракта к реализованному генератору.
