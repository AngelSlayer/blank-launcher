# Blank Launcher

Windows launcher based on Riven 1.3.2, licensed under GPL-3.0-or-later. See the upstream README for credits and platform dependencies.

Install the latest Windows release and open **Blank Launcher** from the Start menu. The **Наши сборки** screen shows the catalog hosted on GitHub Pages. Existing Riven profiles are available in the library.

To add a pack, publish its signed stable channel and add an entry to the catalog in the `AngelSlayer/Blank` repository, branch `gh-pages`, file `catalog.json`. Leave the existing entry IDs unchanged so installed profiles remain associated with their cards.

Upstream: https://github.com/BX-Team/Riven

## Диагностические отчёты

Blank Launcher сохраняет сводку после выхода или сбоя в каталоге данных Riven, в папке `blank-diagnostics`. При включённой диагностике сводка отправляется в приватный Blank-Reports через Cloudflare. При следующем открытии лаунчер повторяет попытку для оставшихся отчётов.

В отчёте есть версия сборки, длительность, тип завершения, категории ошибок, время и число повторов. Личные имена, чат, скриншоты и полные логи остаются на компьютере. Настройка доступна во вкладке «Наши сборки» и в общих настройках.
