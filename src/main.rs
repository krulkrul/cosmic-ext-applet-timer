// SPDX-License-Identifier: GPL-3.0-only

use libcosmic as cosmic;
use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget::{self, list_column};
use cosmic::Element;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::process::Command as TokioCommand;

const APP_ID: &str = "com.krul.CosmicAppletTimer";

// ─── Config ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CustomPreset {
    name: String,
    secs: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct AppConfig {
    pomodoro_work_min: u32,
    pomodoro_short_break_min: u32,
    pomodoro_long_break_min: u32,
    pomodoro_rounds_before_long: u32,
    pomodoro_auto_advance: bool,
    sleep_timer_min: u32,
    sleep_warning_secs: u32,
    eye_rest_interval_min: u32,
    eye_rest_look_secs: u32,
    stretch_interval_min: u32,
    notify_sound: bool,
    notify_desktop: bool,
    last_timer: String,
    custom_presets: Vec<CustomPreset>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            pomodoro_work_min: 25,
            pomodoro_short_break_min: 5,
            pomodoro_long_break_min: 15,
            pomodoro_rounds_before_long: 4,
            pomodoro_auto_advance: false,
            sleep_timer_min: 30,
            sleep_warning_secs: 30,
            eye_rest_interval_min: 20,
            eye_rest_look_secs: 20,
            stretch_interval_min: 60,
            notify_sound: true,
            notify_desktop: true,
            last_timer: "pomodoro".to_string(),
            custom_presets: vec![
                CustomPreset { name: "Coffee".to_string(), secs: 240 },
                CustomPreset { name: "Tea".to_string(), secs: 180 },
                CustomPreset { name: "Meeting".to_string(), secs: 900 },
            ],
        }
    }
}

fn config_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    p.push("cosmic-ext-applet-timer");
    p.push("config.json");
    p
}

fn load_config() -> AppConfig {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &AppConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(s) = serde_json::to_string_pretty(cfg) { let _ = std::fs::write(&path, s); }
}

// ─── Timer types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum TimerKind {
    PomodoroWork,
    ShortBreak,
    LongBreak,
    Sleep,
    /// 20-minute work phase of the 20-20-20 eye rest cycle
    EyeRest,
    /// 20-second look-away phase of the 20-20-20 eye rest cycle
    EyeRestLookAway,
    /// Repeating hourly stretch reminder
    Stretch,
    /// Index into AppConfig::custom_presets
    Custom(usize),
}

impl TimerKind {
    fn display_name(&self, cfg: &AppConfig) -> String {
        match self {
            Self::PomodoroWork    => "Pomodoro".into(),
            Self::ShortBreak      => "Short break".into(),
            Self::LongBreak       => "Long break".into(),
            Self::Sleep           => "Sleep timer".into(),
            Self::EyeRest         => "Eye rest".into(),
            Self::EyeRestLookAway => "Look away!".into(),
            Self::Stretch         => "Stretch break".into(),
            Self::Custom(i)       => cfg.custom_presets.get(*i)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Custom".into()),
        }
    }

    fn total_secs(&self, cfg: &AppConfig) -> u64 {
        match self {
            Self::PomodoroWork    => cfg.pomodoro_work_min as u64 * 60,
            Self::ShortBreak      => cfg.pomodoro_short_break_min as u64 * 60,
            Self::LongBreak       => cfg.pomodoro_long_break_min as u64 * 60,
            Self::Sleep           => cfg.sleep_timer_min as u64 * 60,
            Self::EyeRest         => cfg.eye_rest_interval_min as u64 * 60,
            Self::EyeRestLookAway => cfg.eye_rest_look_secs as u64,
            Self::Stretch         => cfg.stretch_interval_min as u64 * 60,
            Self::Custom(i)       => cfg.custom_presets.get(*i).map(|p| p.secs).unwrap_or(300),
        }
    }

    fn config_key(&self) -> &'static str {
        match self {
            Self::PomodoroWork | Self::ShortBreak | Self::LongBreak => "pomodoro",
            Self::Sleep => "sleep",
            Self::EyeRest | Self::EyeRestLookAway => "eye_rest",
            Self::Stretch => "stretch",
            Self::Custom(_) => "custom",
        }
    }
}

// ─── App state ────────────────────────────────────────────────────────────────

struct RunningTimer {
    kind: TimerKind,
    started: Instant,
    total_secs: u64,
    /// Number of completed pomodoro work sessions (drives short/long break cycling)
    pomo_rounds: u32,
    /// True once the sleep warning notification has been sent
    warned: bool,
}

impl RunningTimer {
    fn new(kind: TimerKind, cfg: &AppConfig, pomo_rounds: u32) -> Self {
        let total_secs = kind.total_secs(cfg);
        Self { kind, started: Instant::now(), total_secs, pomo_rounds, warned: false }
    }

    fn remaining_secs(&self) -> u64 {
        self.total_secs.saturating_sub(self.started.elapsed().as_secs())
    }

    fn is_expired(&self) -> bool {
        self.remaining_secs() == 0
    }
}

enum AppState {
    Idle,
    Running(RunningTimer),
    /// Post-timer grace period before suspend executes; user can still cancel
    SleepGrace { suspend_at: Instant },
}

// ─── Messages ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Message {
    Surface(cosmic::surface::Action),
    PopupClosed(Id),
    ToggleSettings,
    Tick,
    StartTimer(TimerKind),
    StopTimer,
    CancelSleep,
    // Settings
    SetPomodoroWork(u32),
    SetPomodoroShortBreak(u32),
    SetPomodoroLongBreak(u32),
    SetPomodoroRounds(u32),
    TogglePomodoroAutoAdvance,
    SetSleepMin(u32),
    ToggleNotifySound,
    ToggleNotifyDesktop,
    Noop,
}

// ─── AppModel ─────────────────────────────────────────────────────────────────

pub struct AppModel {
    core: Core,
    popup: Option<Id>,
    config: AppConfig,
    state: AppState,
    show_settings: bool,
}

// ─── Async helpers ────────────────────────────────────────────────────────────

async fn fire_notify(title: String, body: String, sound: bool, desktop: bool) -> Message {
    if desktop {
        let _ = TokioCommand::new("notify-send")
            .args(["--app-name=Timer", "--expire-time=8000", "--icon=alarm-symbolic", &title, &body])
            .output()
            .await;
    }
    if sound {
        // Try common freedesktop sound paths; stop at first hit
        for path in &[
            "/usr/share/sounds/freedesktop/stereo/complete.oga",
            "/usr/share/sounds/freedesktop/stereo/bell.oga",
            "/usr/share/sounds/ubuntu/stereo/bell.ogg",
        ] {
            if std::path::Path::new(path).exists() {
                let _ = TokioCommand::new("paplay").arg(path).output().await;
                break;
            }
        }
    }
    Message::Noop
}

async fn execute_suspend() -> Message {
    // Pause all MPRIS-capable media players before suspend
    let _ = TokioCommand::new("playerctl")
        .args(["pause", "--all-players"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let _ = TokioCommand::new("systemctl").arg("suspend").output().await;
    Message::Noop
}

// ─── Format helpers ───────────────────────────────────────────────────────────

fn fmt_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

// ─── Application impl ─────────────────────────────────────────────────────────

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core { &self.core }
    fn core_mut(&mut self) -> &mut Core { &mut self.core }

    fn init(core: Core, _flags: ()) -> (Self, Task<Self::Message>) {
        (
            AppModel {
                core,
                popup: None,
                config: load_config(),
                state: AppState::Idle,
                show_settings: false,
            },
            Task::none(),
        )
    }

    fn on_close_requested(&self, id: cosmic::iced_runtime::core::window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Noop => {}

            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) { self.popup = None; }
            }

            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(action),
                ));
            }

            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
            }

            Message::StartTimer(kind) => {
                self.config.last_timer = kind.config_key().to_string();
                save_config(&self.config);
                self.state = AppState::Running(RunningTimer::new(kind, &self.config, 0));
                self.show_settings = false;
            }

            Message::StopTimer | Message::CancelSleep => {
                self.state = AppState::Idle;
            }

            Message::Tick => {
                return self.process_tick();
            }

            Message::SetPomodoroWork(m) => { self.config.pomodoro_work_min = m; save_config(&self.config); }
            Message::SetPomodoroShortBreak(m) => { self.config.pomodoro_short_break_min = m; save_config(&self.config); }
            Message::SetPomodoroLongBreak(m) => { self.config.pomodoro_long_break_min = m; save_config(&self.config); }
            Message::SetPomodoroRounds(r) => { self.config.pomodoro_rounds_before_long = r; save_config(&self.config); }
            Message::TogglePomodoroAutoAdvance => { self.config.pomodoro_auto_advance = !self.config.pomodoro_auto_advance; save_config(&self.config); }
            Message::SetSleepMin(m) => { self.config.sleep_timer_min = m; save_config(&self.config); }
            Message::ToggleNotifySound => { self.config.notify_sound = !self.config.notify_sound; save_config(&self.config); }
            Message::ToggleNotifyDesktop => { self.config.notify_desktop = !self.config.notify_desktop; save_config(&self.config); }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let (_, v_pad) = self.core.applet.suggested_padding(true);

        let panel_label = match &self.state {
            AppState::Idle => "⏱".to_string(),
            AppState::Running(t) => format!("⏱ {}", fmt_duration(t.remaining_secs())),
            AppState::SleepGrace { suspend_at } => {
                let secs = suspend_at
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                format!("💤 {secs}s")
            }
        };

        let panel_content = widget::row()
            .push(self.core.applet.text(panel_label))
            .align_y(Alignment::Center);

        let have_popup = self.popup;
        let btn = cosmic::widget::button::custom(panel_content)
            .padding([v_pad, 8])
            .class(cosmic::theme::Button::AppletIcon)
            .on_press_with_rectangle(move |_, _| {
                if let Some(id) = have_popup {
                    Message::Surface(destroy_popup(id))
                } else {
                    Message::Surface(app_popup::<AppModel>(
                        move |state: &mut AppModel| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let mut s = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            );
                            s.positioner.size_limits = cosmic::iced::Limits::NONE
                                .min_width(280.0)
                                .max_width(380.0)
                                .min_height(80.0)
                                .max_height(640.0);
                            s
                        },
                        Some(Box::new(|state: &AppModel| {
                            build_popup_view(state).map(cosmic::Action::App)
                        })),
                    ))
                }
            });

        let tooltip = Element::from(self.core.applet.applet_tooltip::<Message>(
            btn,
            "Timer",
            self.popup.is_some(),
            |a| Message::Surface(a),
            None,
        ));
        self.core.applet.autosize_window(tooltip).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        widget::text("").into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        cosmic::iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick)
    }

    fn style(&self) -> Option<cosmic::iced_core::theme::Style> {
        Some(cosmic::applet::style())
    }
}

// ─── Tick logic ───────────────────────────────────────────────────────────────

impl AppModel {
    fn process_tick(&mut self) -> Task<Message> {
        // SleepGrace: execute suspend when countdown reaches zero
        let maybe_grace = if let AppState::SleepGrace { suspend_at } = &self.state {
            Some(*suspend_at)
        } else {
            None
        };
        if let Some(suspend_at) = maybe_grace {
            if Instant::now() >= suspend_at {
                self.state = AppState::Idle;
                return cosmic::task::future(execute_suspend());
            }
            return Task::none();
        }

        // Extract snapshot from Running state; drops the borrow so we can mutate below
        let snapshot = if let AppState::Running(t) = &self.state {
            Some((t.kind.clone(), t.remaining_secs(), t.is_expired(), t.pomo_rounds, t.warned))
        } else {
            None
        };
        let (kind, remaining, is_expired, pomo_rounds, warned) = match snapshot {
            Some(v) => v,
            None => return Task::none(),
        };

        let sound = self.config.notify_sound;
        let desktop = self.config.notify_desktop;
        let auto = self.config.pomodoro_auto_advance;
        let rounds_for_long = self.config.pomodoro_rounds_before_long;
        let warn_secs = self.config.sleep_warning_secs as u64;

        // Sleep warning fires once when ≤ warn_secs remain (before the timer hits zero)
        if kind == TimerKind::Sleep && !warned && remaining <= warn_secs && remaining > 0 {
            if let AppState::Running(t) = &mut self.state {
                t.warned = true;
            }
            return cosmic::task::future(async move {
                fire_notify(
                    "Sleep timer".into(),
                    format!("Suspending in {remaining}s. Cancel from the panel if needed."),
                    sound,
                    desktop,
                )
                .await
            });
        }

        if !is_expired {
            return Task::none();
        }

        // ── Timer expired ────────────────────────────────────────────────────
        match kind {
            TimerKind::Sleep => {
                // Enter 8-second grace: popup shows countdown + Cancel button
                self.state = AppState::SleepGrace {
                    suspend_at: Instant::now() + Duration::from_secs(8),
                };
                cosmic::task::future(async move {
                    fire_notify(
                        "Going to sleep".into(),
                        "Suspending in 8 seconds. Cancel from the panel.".into(),
                        sound,
                        desktop,
                    )
                    .await
                })
            }

            TimerKind::PomodoroWork => {
                let new_rounds = pomo_rounds + 1;
                let long_break = new_rounds % rounds_for_long == 0;
                if auto {
                    let next_kind = if long_break { TimerKind::LongBreak } else { TimerKind::ShortBreak };
                    let next_secs = if long_break {
                        self.config.pomodoro_long_break_min as u64 * 60
                    } else {
                        self.config.pomodoro_short_break_min as u64 * 60
                    };
                    self.state = AppState::Running(RunningTimer {
                        kind: next_kind,
                        started: Instant::now(),
                        total_secs: next_secs,
                        pomo_rounds: new_rounds,
                        warned: false,
                    });
                } else {
                    self.state = AppState::Idle;
                }
                let break_label = if long_break { "long break" } else { "short break" };
                cosmic::task::future(async move {
                    fire_notify(
                        format!("Pomodoro #{new_rounds} done!"),
                        format!("Time for a {break_label}."),
                        sound,
                        desktop,
                    )
                    .await
                })
            }

            TimerKind::ShortBreak | TimerKind::LongBreak => {
                if auto {
                    self.state = AppState::Running(RunningTimer::new(
                        TimerKind::PomodoroWork,
                        &self.config,
                        pomo_rounds,
                    ));
                } else {
                    self.state = AppState::Idle;
                }
                cosmic::task::future(async move {
                    fire_notify("Break over!".into(), "Time to focus.".into(), sound, desktop).await
                })
            }

            TimerKind::EyeRest => {
                // Start the 20-second look-away phase
                let look_secs = self.config.eye_rest_look_secs as u64;
                self.state = AppState::Running(RunningTimer {
                    kind: TimerKind::EyeRestLookAway,
                    started: Instant::now(),
                    total_secs: look_secs,
                    pomo_rounds: 0,
                    warned: false,
                });
                cosmic::task::future(async move {
                    fire_notify(
                        "Eye rest".into(),
                        "Look 20 feet away for 20 seconds.".into(),
                        sound,
                        desktop,
                    )
                    .await
                })
            }

            TimerKind::EyeRestLookAway => {
                // Look-away done; restart the 20-minute work phase
                self.state = AppState::Running(RunningTimer::new(
                    TimerKind::EyeRest,
                    &self.config,
                    0,
                ));
                cosmic::task::future(async move {
                    fire_notify("Eyes rested".into(), "Back to work!".into(), sound, desktop).await
                })
            }

            TimerKind::Stretch => {
                // Repeating: notify then immediately restart
                self.state = AppState::Running(RunningTimer::new(
                    TimerKind::Stretch,
                    &self.config,
                    0,
                ));
                cosmic::task::future(async move {
                    fire_notify(
                        "Stretch time!".into(),
                        "Stand up, move around, stretch for a minute.".into(),
                        sound,
                        desktop,
                    )
                    .await
                })
            }

            TimerKind::Custom(i) => {
                let name = self.config.custom_presets.get(i)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Timer".into());
                self.state = AppState::Idle;
                cosmic::task::future(async move {
                    fire_notify(name, "Done!".into(), sound, desktop).await
                })
            }
        }
    }
}

// ─── Popup views ──────────────────────────────────────────────────────────────

fn build_popup_view(state: &AppModel) -> Element<'_, Message> {
    if state.show_settings {
        return build_settings_view(state);
    }
    match &state.state {
        AppState::Idle => build_preset_view(state),
        AppState::Running(t) => build_running_view(state, t),
        AppState::SleepGrace { suspend_at } => build_sleep_grace_view(state, *suspend_at),
    }
}

// ── Idle: timer selection ────────────────────────────────────────────────────

fn build_preset_view(state: &AppModel) -> Element<'_, Message> {
    let cfg = &state.config;
    let mut content = list_column();

    content = content.add(
        widget::row()
            .push(widget::text("Timer").size(14).width(Length::Fill))
            .push(widget::button::text("⚙").on_press(Message::ToggleSettings))
            .padding([8, 12, 4, 12])
            .align_y(Alignment::Center),
    );

    // Pomodoro
    content = content.add(
        widget::column()
            .push(widget::text("Pomodoro").size(11))
            .padding([6, 12, 2, 12]),
    );
    content = content.add(
        widget::column()
            .push(
                widget::row()
                    .push(widget::button::standard(format!("Work {}m", cfg.pomodoro_work_min))
                        .on_press(Message::StartTimer(TimerKind::PomodoroWork)))
                    .push(widget::button::standard(format!("Break {}m", cfg.pomodoro_short_break_min))
                        .on_press(Message::StartTimer(TimerKind::ShortBreak)))
                    .push(widget::button::standard(format!("Long {}m", cfg.pomodoro_long_break_min))
                        .on_press(Message::StartTimer(TimerKind::LongBreak)))
                    .spacing(4),
            )
            .padding([2, 12]),
    );

    // Wellness
    content = content.add(
        widget::column()
            .push(widget::text("Wellness").size(11))
            .padding([6, 12, 2, 12]),
    );
    content = content.add(
        widget::column()
            .push(
                widget::row()
                    .push(widget::button::standard(format!("Sleep {}m", cfg.sleep_timer_min))
                        .on_press(Message::StartTimer(TimerKind::Sleep)))
                    .push(widget::button::standard("Eye rest")
                        .on_press(Message::StartTimer(TimerKind::EyeRest)))
                    .push(widget::button::standard(format!("Stretch {}m", cfg.stretch_interval_min))
                        .on_press(Message::StartTimer(TimerKind::Stretch)))
                    .spacing(4),
            )
            .padding([2, 12]),
    );

    // Custom presets
    if !cfg.custom_presets.is_empty() {
        content = content.add(
            widget::column()
                .push(widget::text("Custom").size(11))
                .padding([6, 12, 2, 12]),
        );
        let mut row = widget::row().spacing(4);
        for (i, preset) in cfg.custom_presets.iter().enumerate() {
            row = row.push(
                widget::button::standard(format!("{} ({})", preset.name, fmt_duration(preset.secs)))
                    .on_press(Message::StartTimer(TimerKind::Custom(i))),
            );
        }
        content = content.add(widget::column().push(row).padding([2, 12, 8, 12]));
    }

    Element::from(state.core.applet.popup_container(content))
}

// ── Active timer ─────────────────────────────────────────────────────────────

fn build_running_view<'a>(state: &'a AppModel, t: &'a RunningTimer) -> Element<'a, Message> {
    let cfg = &state.config;
    let name = t.kind.display_name(cfg);
    let remaining = t.remaining_secs();

    let sub = match &t.kind {
        TimerKind::PomodoroWork => {
            let round = (t.pomo_rounds % cfg.pomodoro_rounds_before_long) + 1;
            format!("Round {}/{}", round, cfg.pomodoro_rounds_before_long)
        }
        TimerKind::ShortBreak | TimerKind::LongBreak => {
            format!("After round {}", t.pomo_rounds)
        }
        _ => String::new(),
    };

    let mut content = list_column();

    content = content.add(
        widget::row()
            .push(widget::text(name).size(14).width(Length::Fill))
            .push(widget::button::text("⚙").on_press(Message::ToggleSettings))
            .padding([8, 12, 4, 12])
            .align_y(Alignment::Center),
    );

    let has_sub = !sub.is_empty();
    if has_sub {
        content = content.add(
            widget::column()
                .push(widget::text(sub).size(11))
                .padding([0, 12, 4, 12]),
        );
    }

    // Large countdown
    content = content.add(
        widget::column()
            .push(
                widget::row()
                    .push(widget::space::horizontal())
                    .push(widget::text(fmt_duration(remaining)).size(38))
                    .push(widget::space::horizontal()),
            )
            .padding([8, 12, 8, 12]),
    );

    // Inline warning when sleep timer is in its final countdown
    if t.kind == TimerKind::Sleep && t.warned {
        content = content.add(
            widget::column()
                .push(widget::text(format!("⚠ Suspending in {remaining}s")).size(12))
                .padding([0, 12, 4, 12]),
        );
    }

    content = content.add(
        widget::column()
            .push(
                widget::button::standard("■  Stop timer")
                    .on_press(Message::StopTimer)
                    .width(Length::Fill),
            )
            .padding([0, 12, 8, 12]),
    );

    Element::from(state.core.applet.popup_container(content))
}

// ── Sleep grace period ───────────────────────────────────────────────────────

fn build_sleep_grace_view(state: &AppModel, suspend_at: Instant) -> Element<'_, Message> {
    let remaining = suspend_at
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::ZERO)
        .as_secs();

    let content = list_column()
        .add(
            widget::column()
                .push(
                    widget::row()
                        .push(widget::space::horizontal())
                        .push(widget::text("Suspending\u{2026}").size(14))
                        .push(widget::space::horizontal()),
                )
                .padding([12, 12, 4, 12]),
        )
        .add(
            widget::column()
                .push(
                    widget::row()
                        .push(widget::space::horizontal())
                        .push(widget::text(format!("{remaining}s")).size(42))
                        .push(widget::space::horizontal()),
                )
                .padding([4, 12, 8, 12]),
        )
        .add(
            widget::column()
                .push(
                    widget::button::standard("Cancel")
                        .on_press(Message::CancelSleep)
                        .width(Length::Fill),
                )
                .padding([0, 12, 12, 12]),
        );

    Element::from(state.core.applet.popup_container(content))
}

// ── Settings ─────────────────────────────────────────────────────────────────

fn build_settings_view(state: &AppModel) -> Element<'_, Message> {
    let cfg = &state.config;
    let mut content = list_column();

    content = content.add(
        widget::row()
            .push(widget::button::text("\u{2190} Back").on_press(Message::ToggleSettings))
            .push(widget::space::horizontal())
            .push(widget::text("Settings").size(13))
            .padding([8, 12, 4, 12])
            .align_y(Alignment::Center),
    );

    // Pomodoro work duration
    content = content.add(widget::column().push(widget::text("Pomodoro work").size(11)).padding([6, 12, 2, 12]));
    content = content.add(widget::column().push(dur_row(&[15, 20, 25, 30, 45, 60], cfg.pomodoro_work_min, Message::SetPomodoroWork)).padding([2, 12]));

    // Short break
    content = content.add(widget::column().push(widget::text("Short break").size(11)).padding([6, 12, 2, 12]));
    content = content.add(widget::column().push(dur_row(&[3, 5, 10, 15], cfg.pomodoro_short_break_min, Message::SetPomodoroShortBreak)).padding([2, 12]));

    // Long break
    content = content.add(widget::column().push(widget::text("Long break").size(11)).padding([6, 12, 2, 12]));
    content = content.add(widget::column().push(dur_row(&[10, 15, 20, 30], cfg.pomodoro_long_break_min, Message::SetPomodoroLongBreak)).padding([2, 12]));

    // Rounds before long break
    content = content.add(widget::column().push(widget::text("Rounds before long break").size(11)).padding([6, 12, 2, 12]));
    {
        let cur = cfg.pomodoro_rounds_before_long;
        let mut row = widget::row().spacing(4);
        for &r in &[2u32, 3, 4, 5, 6] {
            let label = if r == cur { format!("\u{2713}{r}") } else { r.to_string() };
            row = row.push(
                widget::button::standard(label)
                    .on_press_maybe(if r != cur { Some(Message::SetPomodoroRounds(r)) } else { None }),
            );
        }
        content = content.add(widget::column().push(row).padding([2, 12]));
    }

    // Auto-advance toggle
    {
        let label = if cfg.pomodoro_auto_advance { "Auto-advance phases: ON" } else { "Auto-advance phases: OFF" };
        content = content.add(
            widget::column()
                .push(widget::button::standard(label).on_press(Message::TogglePomodoroAutoAdvance).width(Length::Fill))
                .padding([4, 12]),
        );
    }

    // Sleep timer duration
    content = content.add(widget::column().push(widget::text("Sleep timer").size(11)).padding([6, 12, 2, 12]));
    content = content.add(widget::column().push(dur_row(&[15, 20, 30, 45, 60, 90, 120], cfg.sleep_timer_min, Message::SetSleepMin)).padding([2, 12]));

    // Notifications
    content = content.add(widget::column().push(widget::text("Notifications").size(11)).padding([6, 12, 2, 12]));
    {
        let s = if cfg.notify_sound { "Sound: ON" } else { "Sound: OFF" };
        let d = if cfg.notify_desktop { "Desktop: ON" } else { "Desktop: OFF" };
        content = content.add(
            widget::column()
                .push(
                    widget::row()
                        .push(widget::button::standard(s).on_press(Message::ToggleNotifySound).width(Length::Fill))
                        .push(widget::button::standard(d).on_press(Message::ToggleNotifyDesktop).width(Length::Fill))
                        .spacing(4),
                )
                .padding([2, 12, 8, 12]),
        );
    }

    Element::from(state.core.applet.popup_container(widget::scrollable(content)))
}

/// Build a row of duration-choice buttons; the active value is grayed (non-clickable).
fn dur_row(values: &[u32], current: u32, make_msg: fn(u32) -> Message) -> widget::Row<'_, Message> {
    let mut row = widget::row().spacing(4);
    for &v in values {
        let label = if v == current { format!("\u{2713}{v}m") } else { format!("{v}m") };
        row = row.push(
            widget::button::standard(label)
                .on_press_maybe(if v != current { Some(make_msg(v)) } else { None }),
        );
    }
    row
}

// ─── Entry point ──────────────────────────────────────────────────────────────

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<AppModel>(())
}
