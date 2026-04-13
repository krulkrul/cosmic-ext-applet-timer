# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & deploy

```bash
cargo build                   # debug build
cargo build --release         # release build (size-optimised via Cargo.toml profile)
cargo clippy                  # lint
./setup.sh                    # first-time: release build + install desktop entry
./dev-reload.sh               # subsequent: release build + restart cosmic-panel
```

Run directly (requires a COSMIC/Wayland session):
```bash
./target/debug/cosmic-ext-applet-timer
```

### Desktop entry

`setup.sh` writes `~/.local/share/applications/com.krul.CosmicAppletTimer.desktop` with `Exec` pointing at `$(pwd)/target/release/cosmic-ext-applet-timer`. The required fields are `NoDisplay=true` and `X-CosmicApplet=true` (camelCase — `X-COSMIC-Applet` is wrong and will not show up in the panel).

## Purpose

Panel applet for timers: Pomodoro (with full short/long break cycling), sleep timer (suspends the laptop), 20-20-20 eye rest, stretch reminder, and custom presets. Follows the exact same patterns as sibling applets `../cosmic-ext-applet-crypto` and `../cosmic-ext-applet-spotify`.

## Architecture

Single-file Rust applet (`src/main.rs`, ~880 lines) using **libcosmic** pinned to rev `c52ef976`. The `Cargo.lock` was seeded from `../cosmic-ext-applet-spotify/Cargo.lock`.

### State machine

```
AppState::Idle
  → Running(RunningTimer)   on StartTimer
  → SleepGrace { suspend_at }  when Sleep timer expires (8s cancel window)
  → Idle                    on StopTimer / CancelSleep
```

`RunningTimer` holds `started: Instant` and `total_secs`; remaining is computed on each tick via `total_secs.saturating_sub(elapsed)`. `Instant` is not persisted — timers are forgotten on restart, but `config.last_timer` remembers which preset was last used.

### Tick logic (`AppModel::process_tick`)

Subscription fires every second (`cosmic::iced::time::every(Duration::from_secs(1))`). Each tick:
1. If `SleepGrace` and `Instant::now() >= suspend_at`: call `execute_suspend()` async (pauses media via `playerctl`, then `systemctl suspend`)
2. If `Running(Sleep)` and `remaining <= sleep_warning_secs` and not yet warned: set `warned = true`, fire notification
3. If expired: handle by kind — Sleep → SleepGrace, PomodoroWork → ShortBreak/LongBreak (or Idle if auto-advance off), EyeRest → EyeRestLookAway → EyeRest cycle, Stretch → restart, Custom → Idle

### Key libcosmic patterns

**Surface action type** — use `cosmic::surface::Action` (not `cosmic::surface::action::Action` which is private).

**Popup toggle** — `on_press_with_rectangle(move |_, _| { ... })` returns either `destroy_popup(id)` or `app_popup::<AppModel>(init_fn, Some(Box::new(content_fn)))`, both wrapped in `Message::Surface(...)`.

**Async tasks** — `cosmic::task::future(async move { ...; Message::Noop })` for fire-and-forget side effects (notifications, suspend).

**Borrow pattern** — extract snapshot from `&self.state` in a block (drops borrow), then mutate `self.state`. This avoids simultaneous mutable + immutable borrows of `self`.

**Popup content functions** — all `build_*_view(state: &AppModel)` return `Element<'_, Message>` and end with `Element::from(state.core.applet.popup_container(content))`.

**Settings UI** — `dur_row(values, current, make_msg_fn)` renders a row of buttons; the active value has `on_press_maybe(None)` (appears disabled = selected) and a `✓` prefix.

### Config

`AppConfig` → `~/.config/cosmic-ext-applet-timer/config.json`. `save_config` called inline in every settings message handler. No complex config types — durations in minutes, booleans for toggles.

### App ID

`com.krul.CosmicAppletTimer`

### Cargo.toml profile

```toml
[profile.release]
opt-level = "s"
lto = true
```
