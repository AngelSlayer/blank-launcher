use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, relative,
};
use riven_format::Changes;
use rust_i18n::t;

use super::{runtime, state::AppState};
use moonglass::theme::ActiveTheme as _;
use moonglass::{Button, ButtonSize, v_flex};

const HISTORY_LIMIT: usize = 5;

/// Opens the recent release notes and the last update's file summary.
pub fn open(id: &str, version: String, changes: Option<Changes>, cx: &mut App) {
    let source = AppState::global(cx)
        .read(cx)
        .store
        .as_ref()
        .and_then(|store| {
            riven_sync::install::load_state(&store.game_dir(id))
                .ok()
                .flatten()
        })
        .map(|state| state.source);
    let view = cx.new(|cx| {
        let mut dialog = ChangesDialog {
            version,
            changes: changes.unwrap_or_default(),
            loading: source.is_some(),
        };
        dialog.changes.notes.truncate(HISTORY_LIMIT);
        if let Some(source) = source {
            let version = dialog.version.clone();
            cx.spawn(async move |this, cx| {
                let result = runtime::spawn(async move {
                    let store = riven_sync::Store::default_location()?;
                    let release =
                        riven_sync::update::preview(&store, &riven_sources::client(), &source, None)
                            .await
                            .ok()?;
                    let notes: Vec<_> = release
                        .notes
                        .into_iter()
                        .skip_while(|note| note.version != version)
                        .take(HISTORY_LIMIT)
                        .collect();
                    (!notes.is_empty()).then_some(notes)
                })
                .await;
                let _ = this.update(cx, |dialog, cx| {
                    dialog.loading = false;
                    if let Ok(Some(notes)) = result {
                        dialog.changes.notes = notes;
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        dialog
    });
    super::dialogs::open(view.into(), 520., cx);
}

struct ChangesDialog {
    version: String,
    changes: Changes,
    loading: bool,
}

fn section(title: SharedString, names: &[String], cx: &App) -> Option<AnyElement> {
    if names.is_empty() {
        return None;
    }
    let c = cx.theme().colors;
    Some(
        v_flex()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.muted)
                    .child(format!("{title} · {}", names.len())),
            )
            .child(div().line_height(relative(1.5)).child(names.join(", ")))
            .into_any_element(),
    )
}

impl Render for ChangesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let ch = &self.changes;
        let notes = ch.notes.iter().map(|n| {
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(n.version.clone()),
                )
                .child(
                    div()
                        .line_height(relative(1.5))
                        .text_color(c.text2)
                        .child(n.text.clone()),
                )
        });
        let sections = [
            section(t!("changes.added").into(), &ch.added, cx),
            section(t!("changes.updated").into(), &ch.updated, cx),
            section(t!("changes.removed").into(), &ch.removed, cx),
        ];
        let has_diff = ch.other > 0 || sections.iter().any(Option::is_some);
        let body = v_flex()
            .id("pack-changes")
            .max_h(px(380.))
            .overflow_y_scroll()
            .gap(px(14.))
            .when(self.loading, |b| {
                b.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("changes.loading").to_string()),
                )
            })
            .children(notes)
            .when(has_diff, |b| {
                b.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("changes.from", version = ch.from).to_string()),
                )
            })
            .children(sections.into_iter().flatten())
            .when(ch.other > 0, |b| {
                b.child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .child(t!("changes.other", n = ch.other).to_string()),
                )
            })
            .when(!self.loading && ch.notes.is_empty() && !has_diff, |b| {
                b.child(
                    div()
                        .text_color(c.muted)
                        .child(t!("changes.nothing").to_string()),
                )
            });
        let close = Button::new("close")
            .outline()
            .size(ButtonSize::Md)
            .label(t!("common.close"))
            .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.close_modal(cx)));
        super::dialogs::dialog_shell(
            t!("changes.title", version = self.version),
            t!("changes.history"),
            body,
            vec![close],
            cx,
        )
    }
}
