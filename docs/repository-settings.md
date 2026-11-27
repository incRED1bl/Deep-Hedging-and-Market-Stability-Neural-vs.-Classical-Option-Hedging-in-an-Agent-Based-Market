# Настройки репозитория

**Статус на 27 ноября 2026:** защита `main` ещё не включена. Ниже — целевые настройки; наличие этого файла и CI в PR само по себе их не включает. Применение требует доступа владельца к настройкам GitHub.

## Защита `main`

Settings → Rules → Rulesets: создать активный branch ruleset для `refs/heads/main`.

- [ ] Enforcement: **Active**; список bypass пуст, включая владельца и приложения.
- [ ] Требовать PR и **1 approving review**.
- [ ] Включить **Dismiss stale pull request approvals when new commits are pushed**.
- [ ] Включить **Require approval of the most recent reviewable push**.
- [ ] Включить **Require review from Code Owners**.
- [ ] Требовать разрешения всех обсуждений.
- [ ] Требовать успешные проверки **`lint`** и **`test`**.
- [ ] Требовать актуальность ветки относительно `main` перед слиянием — strict status checks.
- [ ] Для обеих проверок выбрать источник **GitHub Actions**, а не Any source. Числовой `integration_id` для API взять из `app.id` реального check run; заранее его не подставлять.
- [ ] Запретить force push и удаление `main`.

Если применяется классическая branch protection вместо ruleset: включить **Do not allow bypassing the above settings** (`enforce_admins: true`). Дублировать две схемы защиты не требуется.

## Слияние и Actions

- [ ] Settings → General → Pull Requests: оставить только **Squash merging**; отключить merge commits и rebase merging.
- [ ] Включить **Automatically delete head branches**.
- [ ] Settings → Actions → General → Workflow permissions: **Read repository contents and packages permissions**.
- [ ] Отключить **Allow GitHub Actions to create and approve pull requests**.
- [ ] В workflow сохранить `permissions: contents: read`, установку через `uv sync --locked` и отдельные jobs `lint` / `test`.
- [ ] Не добавлять секреты проекта для обычного CI: линтеру и CPU-тестам они не нужны.

## CODEOWNERS и первый PR

В текущем CODEOWNERS указаны подтверждённые `@incRED1bl` (admin) и `@Levonidor` (write), включая инфраструктуру и сам CODEOWNERS. Для каждого модуля затем уточняем основного и резервного ревьюера. Точные логины участников, обозначенных как `bani` и `perfy`, ещё требуют проверки: эти буквальные логины возвращают только `read`; предполагаемые аккаунты в CODEOWNERS не добавляем.

Оба владельца пути записываются **на одной строке**. Достаточно одобрения **любого одного** из них: это не требование двух одобрений и не гарантия обязательного одобрения Тимом. Резервный ревьюер нужен в том числе для PR самого Тима: автор не может одобрить собственный PR. Сам файл `.github/CODEOWNERS` также должен иметь владельцев.

Порядок для первого инфраструктурного PR:

1. Дождаться запуска `lint` и `test`; проверить их фактические имена и приложение-источник.
2. Проверить точные логины, принятие приглашений и `write`-доступ ревьюеров.
3. Включить правила защиты с проверенными именами checks и источником.
4. Вручную запросить review у участника, который не является автором и не сделал последний push.
5. Слить через squash только после review и успешных checks.

GitHub берёт CODEOWNERS из **целевой ветки PR**. Поэтому файл, впервые добавляемый в PR, ещё не назначит ревьюеров этому PR автоматически. Автоматические запросы также не отправляются для draft PR.

## Границы прав

У личного репозитория есть владелец и collaborators. Управление доступом и защитой репозитория остаётся у владельца; collaborators работают с кодом и PR. При этом collaborators могут создавать repository Actions secrets и variables. Поэтому нельзя обещать, что им недоступен вообще весь раздел Settings. Управлять environment secrets личного репозитория может владелец.

Пустой bypass запрещает обход требований при слиянии, но не лишает владельца возможности изменить сами правила.

## Документация

- [Rulesets и параметры API](https://docs.github.com/en/rest/repos/rules)
- [CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners)
- [Права в личном репозитории](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository)
- [Права на Actions secrets](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets)
