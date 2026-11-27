# Настройки репозитория

27 ноября 2026 GitHub API подтвердил, что для `refs/heads/main` действует
[ruleset Protect main](https://github.com/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market/rules/28249578).
[main-ruleset.json](main-ruleset.json) описывает первый этап защиты.

## Применено

- Enforcement `active`. Список bypass пуст, включая владельца и приложения.
- Изменения через PR. Нужны два независимых одобрения, review от
  CODEOWNERS, одобрение последнего reviewable push другим участником и
  разрешение всех обсуждений. Новый push сбрасывает прежние approvals.
- Обязательны `lint` и `test` от GitHub Actions (`integration_id: 15368`).
  Ветка PR должна быть актуальной относительно `main`.
- Запрещены force push и удаление `main`. Требуется линейная история.
- Разрешён только squash merge. GitHub автоматически удаляет head-ветку после
  слияния, если её можно удалить.
- Actions по умолчанию получают read-only права. Создание и одобрение PR через
  `GITHUB_TOKEN` отключены. Python CI использует `contents: read`, actions по
  commit SHA, `persist-credentials: false` и таймауты. Секреты ему не нужны.

Тим (@incRED1bl) — владелец с `admin`. Даня (@bani2016up), Федя
(@Perf0rator4) и Лев (@Levonidor) — collaborators с `write`. Управление
доступом и правилами остаётся у владельца. В личном репозитории collaborators
могут управлять repository Actions secrets и variables. Environment secrets
управляет владелец. Владелец также может редактировать сам ruleset.

## CODEOWNERS

GitHub берёт CODEOWNERS из целевой ветки PR. Общее правило в
[`.github/CODEOWNERS`](../.github/CODEOWNERS) даёт Тиму, Дане, Феде и Льву
одинаковое право одобрять изменения во всех файлах. Для слияния в `main`
нужны два Approve от коллег из команды. Одобрение последнего reviewable push
даёт другой участник. Автор PR с правом `write` может сам выполнить squash merge
после одобрений и проверок. Предметных reviewers выбираем по issue и
[CONTRIBUTING.md](../CONTRIBUTING.md#ответственные-и-review).

## После подключения Rust CI

[Rust workflow в PR #25](https://github.com/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market/pull/25)
уже дал успешный `rust-checks` от GitHub Actions. Обязательная проверка
ожидает интеграции workflow в `main`.

1. После слияния #22 обновить #25 от `main`. На актуальном head должны пройти
   `lint`, `test` и `rust-checks`. Сохранить в Rust workflow запуск на каждом
   PR без фильтра по путям и веткам, добавить разумный таймаут job.
2. После review и разрешённого squash merge #25 дождаться успешного запуска
   Rust workflow на `main`. Проверить запуск на всех открытых PR. При
   необходимости обновить их ветки и дождаться появления check run.
3. Добавить `rust-checks` к действующему ruleset командой ниже. Cargo fmt,
   Clippy, тесты и пороги 100% по строкам и функциям уже входят в этот job.
   Его неуспешный результат после включения будет блокировать merge в `main`.

Команда для владельца проверяет наличие workflow в `main`, источник
`rust-checks` и появление проверки на текущих head открытых PR. Она сохраняет
остальные правила и при повторном запуске не добавляет дубликат.

```bash
bash -euo pipefail <<'BASH'
repo='repos/incRED1bl/Deep-Hedging-and-Market-Stability-Neural-vs.-Classical-Option-Hedging-in-an-Agent-Based-Market'
ruleset="$repo/rulesets/28249578"
workflow="$repo/actions/workflows/rust-checks.yml"

gh api "$repo/contents/.github/workflows/rust-checks.yml?ref=main" --silent
gh api "$workflow" | jq -e '.state == "active"' >/dev/null
run=$(gh api "$workflow/runs?event=push&branch=main&status=success&per_page=1" \
  | jq -er '.workflow_runs[0].id')
check=$(gh api "$repo/actions/runs/$run/jobs" \
  | jq -er '.jobs[] | select(.name == "rust-checks" and .conclusion == "success") | .check_run_url')
gh api "$check" | jq -e '.app.slug == "github-actions" and .app.id == 15368' >/dev/null

pulls=$(gh api --paginate --slurp "$repo/pulls?state=open&per_page=100" \
  | jq -r '.[][] | [.number, .head.sha] | @tsv')
while IFS=$'\t' read -r number sha; do
  [[ -z "$number" ]] && continue
  gh api "$repo/commits/$sha/check-runs?check_name=rust-checks" \
    | jq -e 'any(.check_runs[]; .app.id == 15368)' >/dev/null \
    || { echo "PR #$number: rust-checks от GitHub Actions пока отсутствует" >&2; exit 1; }
done <<< "$pulls"

current=$(gh api "$ruleset")
gate='{"context":"rust-checks","integration_id":15368}'
payload=$(jq -ec --argjson gate "$gate" '
  select(.target == "branch" and .enforcement == "active" and .bypass_actors == []
    and .conditions.ref_name == {"include":["refs/heads/main"],"exclude":[]})
  | {name,target,enforcement,bypass_actors,conditions,rules}
  | (.rules[] | select(.type == "required_status_checks"
      and .parameters.strict_required_status_checks_policy)
    | .parameters.required_status_checks) |= (
      if any(.[]; .context == $gate.context and .integration_id != $gate.integration_id)
      then error("rust-checks has a different source")
      elif index($gate) then . else . + [$gate] end
    )
  | select(any(.rules[]; .type == "required_status_checks"
      and .parameters.strict_required_status_checks_policy
      and (.parameters.required_status_checks | index($gate)) != null))
' <<< "$current")
if ! jq -e --argjson gate "$gate" 'any(.rules[]; .type == "required_status_checks"
  and (.parameters.required_status_checks | index($gate)) != null)' <<< "$current" >/dev/null; then
  gh api --method PUT "$ruleset" --input - --silent <<< "$payload"
fi
gh api "$repo/rules/branches/main" --jq '.[] | select(.type == "required_status_checks").parameters'
BASH
```

## Документация GitHub

- [Rulesets и параметры API](https://docs.github.com/en/rest/repos/rules)
- [CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners)
- [Права в личном репозитории](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository)
- [Права на Actions secrets](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets)
