use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, relative,
};
use riven_launch::update::{self, Edition, Install, Release};
use rust_i18n::t;
use tokio::sync::mpsc;

use super::dialogs;
use super::launch_bar::progress_bar;
use super::markdown::{self, Block};
use super::runtime;
use super::state::AppState;
use moonglass::theme::ActiveTheme as _;
use moonglass::toast::ToastKind;
use moonglass::{Button, ButtonSize, IconName, v_flex};

/// Where the search for a new Riven stands.
#[derive(Debug, Clone, Default)]
pub enum Update {
    #[default]
    Idle,
    Checking,
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
        total: u64,
    },
    Failed {
        release: Release,
        error: SharedString,
    },
}

impl Update {
    fn release(&self) -> Option<&Release> {
        match self {
            Update::Idle | Update::Checking => None,
            Update::Available(release)
            | Update::Downloading { release, .. }
            | Update::Failed { release, .. } => Some(release),
        }
    }
}

impl AppState {
    /// Asks GitHub for a newer release; `manual` reports "up to date" and errors too.
    pub fn check_update(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.update, Update::Checking | Update::Downloading { .. }) {
            return;
        }
        let before = std::mem::replace(&mut self.update, Update::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(update::check())
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Update("cancelled".into())));
            let _ = this.update(cx, |s, cx| {
                s.update = match found {
                    Ok(Some(release)) => Update::Available(release),
                    Ok(None) => {
                        if manual {
                            moonglass::toast::show(
                                ToastKind::Success,
                                t!("update.latest", version = update::CURRENT),
                                cx,
                            );
                        }
                        Update::Idle
                    }
                    Err(e) => {
                        tracing::warn!("update check failed: {e}");
                        moonglass::toast::show(
                            ToastKind::Error,
                            t!("update.check_failed", error = e),
                            cx,
                        );
                        before
                    }
                };
                if matches!(s.update, Update::Available(_)) {
                    s.install_update(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads the found release, puts it in place and restarts into it.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        if !Install::detect().self_updates() || self.sessions.values().any(super::session::Session::busy) {
            return;
        }
        let release = match &self.update {
            Update::Available(release) | Update::Failed { release, .. } => release.clone(),
            _ => return,
        };
        self.update = Update::Downloading {
            release: release.clone(),
            done: 0,
            total: 0,
        };
        cx.notify();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let job = runtime::spawn({
            let release = release.clone();
            async move {
                let progress = |done, total| {
                    let _ = tx.send((done, total));
                };
                update::install(&release, &Install::detect(), Edition::Launcher, progress).await
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some((done, total)) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    if let Update::Downloading {
                        done: d, total: t, ..
                    } = &mut s.update
                    {
                        (*d, *t) = (done, total);
                        cx.notify();
                    }
                });
            }
            let result = job
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Update("cancelled".into())));
            let error = match result.map(|relaunch| relaunch.spawn()) {
                Ok(Ok(())) => {
                    cx.update(|cx| cx.quit());
                    return;
                }
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            tracing::error!("update failed: {error}");
            let _ = this.update(cx, |s, cx| {
                s.update = Update::Failed {
                    release,
                    error: error.into(),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

/// Check before showing the library, including when older preferences disabled updates.
pub fn check_at_startup(cx: &mut App) {
    if !cfg!(debug_assertions) {
        AppState::global(cx).update(cx, |s, cx| s.check_update(false, cx));
    }
}

pub(super) fn blocking_screen(
    state: &gpui::Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let update = state.read(cx).update.clone();
    let (title, detail, fraction, failed) = match update {
        Update::Idle => return None,
        Update::Checking => (
            "Проверяем обновления".to_owned(),
            "Проверяем последнюю версию Blank Launcher на GitHub.".to_owned(),
            None,
            false,
        ),
        Update::Available(release) => (
            format!("Доступен Blank Launcher {}", release.version),
            "Обновление начнётся после завершения запущенной игры или установки сборки.".to_owned(),
            None,
            false,
        ),
        Update::Downloading { release, done, total } => {
            let fraction = (total > 0).then(|| (done as f32 / total as f32).clamp(0., 1.));
            let detail = if total > 0 && done >= total {
                "Файл скачан. Проверяем и устанавливаем обновление.".to_owned()
            } else if let Some(fraction) = fraction {
                format!("Скачиваем обновление… {:.0}%", fraction * 100.)
            } else {
                "Начинаем скачивание обновления…".to_owned()
            };
            (format!("Обновление до {}", release.version), detail, fraction, false)
        }
        Update::Failed { release, error } => (
            format!("Не удалось обновиться до {}", release.version),
            error.to_string(),
            None,
            true,
        ),
    };
    let c = cx.theme().colors;
    let mut panel = v_flex().w_full().max_w(px(480.)).gap(px(16.))
        .p(px(20.))
        .child(div().text_size(px(24.)).child(title))
        .child(div().text_color(c.muted).child(detail));
    if failed {
        let state = state.clone();
        panel = panel.child(
            Button::new("retry-required-update")
                .primary()
                .label("Повторить загрузку")
                .on_click(move |_, _, cx| state.update(cx, |s, cx| s.install_update(cx))),
        );
    } else {
        panel = panel.child(progress_bar("required-update-progress", fraction, window, cx));
    }
    Some(
        v_flex().size_full().bg(c.bg)
            .child(moonglass::TitleBar::new("Blank Launcher").logo(super::assets::LOGO))
            .child(div().flex_1().flex().items_center().justify_center().child(panel))
            .children(moonglass::chrome::resize_edges(window))
            .into_any_element(),
    )
}

/// The title bar button that appears once a new version is found.
pub fn title_button(cx: &App) -> Option<AnyElement> {
    let update = &AppState::global(cx).read(cx).update;
    let release = update.release()?;
    let label = match update {
        Update::Downloading { done, total, .. } if *total > 0 => {
            t!("update.percent", percent = done * 100 / total).to_string()
        }
        Update::Downloading { .. } => t!("update.installing").to_string(),
        _ => t!("update.button").to_string(),
    };
    Some(
        Button::new("update")
            .primary()
            .icon(IconName::ArrowDown)
            .label(label)
            .tooltip(t!("update.available_title", version = release.version))
            .on_click(|_, _, cx| open(cx))
            .into_any_element(),
    )
}

pub fn open(cx: &mut App) {
    let view = cx.new(UpdateDialog::new);
    dialogs::open(view.into(), 520., cx);
}

/// The new version's notes and the button that installs it.
pub struct UpdateDialog {
    install: Install,
    notes: Vec<Block>,
    _state: Subscription,
}

impl UpdateDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let notes = state
            .read(cx)
            .update
            .release()
            .map(|r| markdown::blocks(&changelog(&r.notes)))
            .unwrap_or_default();
        Self {
            install: Install::detect(),
            notes,
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for UpdateDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let state = AppState::global(cx);
        let st = state.read(cx);
        let update = st.update.clone();
        let playing = st.sessions.values().any(|s| s.busy());
        let Some(release) = update.release().cloned() else {
            return dialogs::dialog_shell(
                t!("update.check"),
                t!("update.latest", version = update::CURRENT),
                div(),
                vec![close_button(t!("common.close"))],
                cx,
            );
        };
        let page = release.page.clone();
        let notes = if self.notes.is_empty() {
            vec![
                div()
                    .text_color(c.muted)
                    .child(t!("update.no_notes").to_string())
                    .into_any_element(),
            ]
        } else {
            markdown::view(&self.notes, cx)
        };
        let status = match &update {
            Update::Downloading { done, total, .. } => Some(
                v_flex()
                    .gap(px(6.))
                    .child(progress_bar(
                        "update",
                        (*total > 0).then(|| *done as f32 / *total as f32),
                        window,
                        cx,
                    ))
                    .child(
                        div().text_color(c.muted).text_size(px(12.)).child(
                            t!(
                                "update.downloading",
                                done = format!("{:.1}", *done as f64 / 1e6),
                                total = format!("{:.1}", *total as f64 / 1e6)
                            )
                            .to_string(),
                        ),
                    )
                    .into_any_element(),
            ),
            Update::Failed { error, .. } => Some(
                div()
                    .text_color(moonglass::theme::danger())
                    .line_height(relative(1.5))
                    .child(t!("update.failed", error = error).to_string())
                    .into_any_element(),
            ),
            _ => None,
        };
        let blocker = if !self.install.self_updates() {
            Some(t!("update.managed"))
        } else if playing {
            Some(t!("update.game_running"))
        } else {
            None
        };
        let body = v_flex()
            .gap(px(6.))
            .gap(px(14.))
            .child(
                div()
                    .id("update-notes")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(notes),
            )
            .children(status)
            .when_some(blocker.clone(), |col, text| {
                col.child(div().text_color(c.warn).child(text.to_string()))
            });
        let downloading = matches!(update, Update::Downloading { .. });
        let mut buttons = vec![
            Button::new("release-page")
                .outline()
                .size(ButtonSize::Md)
                .icon(IconName::ExternalLink)
                .label(t!("update.release_page"))
                .on_click(move |_, _, cx| cx.open_url(&page)),
            close_button(t!("update.later")),
        ];
        if self.install.self_updates() {
            buttons.push(
                Button::new("install-update")
                    .primary()
                    .size(ButtonSize::Md)
                    .label(if downloading {
                        t!("update.installing")
                    } else {
                        t!("update.install")
                    })
                    .disabled(downloading || blocker.is_some())
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |s, cx| s.install_update(cx))
                    }),
            );
        }
        dialogs::dialog_shell(
            t!("update.available_title", version = release.version),
            t!("update.current", version = update::CURRENT),
            body,
            buttons,
            cx,
        )
    }
}

fn close_button(label: impl Into<SharedString>) -> Button {
    Button::new("close-update")
        .outline()
        .size(ButtonSize::Md)
        .label(label)
        .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)))
}

/// Release notes without their "Downloads" section: the file table is for the website.
fn changelog(notes: &str) -> String {
    let mut out = Vec::new();
    let mut downloads = false;
    for line in notes.lines() {
        let heading = line.trim_start();
        if heading.starts_with('#') {
            let title = heading
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .trim();
            downloads = title.eq_ignore_ascii_case("downloads");
        }
        if !downloads {
            out.push(line);
        }
    }
    out.join("\n")
}

/// Shows the dialog for a known update, else looks for one.
pub fn check_or_open(cx: &mut App) {
    let state = AppState::global(cx);
    if state.read(cx).update.release().is_some() {
        open(cx);
    } else {
        state.update(cx, |s, cx| s.check_update(true, cx));
    }
}

/// The notes of the running version, fetched when the dialog opens.
enum Notes {
    Loading,
    Ready { blocks: Vec<Block>, page: String },
    Missing,
    Failed(String),
}

/// "What's new" in the version that is running.
pub struct NotesDialog {
    notes: Notes,
}

impl NotesDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let found = runtime::spawn(update::release(update::CURRENT))
                .await
                .unwrap_or_else(|_| Err(riven_launch::LaunchError::Update("cancelled".into())));
            let _ = this.update(cx, |this, cx| {
                this.notes = match found {
                    Ok(Some(release)) => Notes::Ready {
                        blocks: markdown::blocks(&changelog(&release.notes)),
                        page: release.page,
                    },
                    Ok(None) => Notes::Missing,
                    Err(e) => Notes::Failed(e.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
        Self {
            notes: Notes::Loading,
        }
    }
}

impl Render for NotesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let line = |text: String, color| div().text_color(color).child(text).into_any_element();
        let (body, page) = match &self.notes {
            Notes::Loading => (
                vec![line(t!("update.notes_loading").to_string(), c.muted)],
                None,
            ),
            Notes::Ready { blocks, page } if blocks.is_empty() => (
                vec![line(t!("update.no_notes").to_string(), c.muted)],
                Some(page.clone()),
            ),
            Notes::Ready { blocks, page } => (markdown::view(blocks, cx), Some(page.clone())),
            Notes::Missing => (
                vec![line(t!("update.notes_missing").to_string(), c.muted)],
                None,
            ),
            Notes::Failed(error) => (
                vec![line(
                    t!("update.notes_failed", error = error).to_string(),
                    c.warn,
                )],
                None,
            ),
        };
        let mut buttons = Vec::new();
        if let Some(page) = page {
            buttons.push(
                Button::new("notes-page")
                    .outline()
                    .size(ButtonSize::Md)
                    .icon(IconName::ExternalLink)
                    .label(t!("update.release_page"))
                    .on_click(move |_, _, cx| cx.open_url(&page)),
            );
        }
        buttons.push(close_button(t!("common.close")));
        dialogs::dialog_shell(
            t!("update.whats_new", version = update::CURRENT),
            t!("update.whats_new_hint"),
            div()
                .id("notes")
                .max_h(px(360.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(body),
            buttons,
            cx,
        )
    }
}

pub fn open_notes(cx: &mut App) {
    let view = cx.new(NotesDialog::new);
    dialogs::open(view.into(), 560., cx);
}

#[cfg(test)]
mod tests {
    use super::changelog;

    #[test]
    fn notes_lose_the_downloads_section_only() {
        let notes = "## 🐛 Fixes\n\n- a fix\n\n## 📦 Downloads\n\n| System | File |\n| --- | --- |\n\
                     | Arch | `x.pkg` |\n\nThe macOS app is not notarized.\n\n## ✨ Features\n\n- a feature";
        assert_eq!(
            changelog(notes),
            "## 🐛 Fixes\n\n- a fix\n\n## ✨ Features\n\n- a feature"
        );
    }
}
