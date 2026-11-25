use gpui::{AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use moonglass::{Button, v_flex};
use serde_json::Value;

use super::{launch_bar, new_instance, runtime, state::{AppState, Route}};

const CATALOG: &str = "https://angelslayer.github.io/Blank/catalog.json";

struct Pack {
    id: String,
    name: String,
    description: String,
    link: String,
}

enum CatalogState {
    Loading,
    Ready(Vec<Pack>),
    Unavailable,
}

pub(super) struct Catalog(CatalogState);

impl Catalog {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let mut catalog = Self(CatalogState::Loading);
        catalog.refresh(cx);
        catalog
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.0 = CatalogState::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(fetch_catalog()).await;
            let _ = this.update(cx, |this, cx| {
                this.0 = match result {
                    Ok(Some(packs)) => CatalogState::Ready(packs),
                    _ => CatalogState::Unavailable,
                };
                cx.notify();
            });
        }).detach();
    }
}

async fn fetch_catalog() -> Option<Vec<Pack>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build().ok()?;
    let document: Value = client.get(CATALOG)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    if document["format"].as_u64() != Some(1) {
        return None;
    }
    let entries = document["packs"].as_array()?;
    let mut seen = std::collections::HashSet::new();
    Some(entries.iter().filter_map(|entry| {
        let id = entry["id"].as_str()?;
        let name = entry["name"].as_str()?;
        let link = entry["url"].as_str()?;
        let url = url::Url::parse(link).ok()?;
        if id.is_empty() || name.is_empty() || url.scheme() != "https"
            || url.host_str() != Some("angelslayer.github.io")
            || !url.username().is_empty() || url.password().is_some()
            || !seen.insert(id.to_owned()) {
            return None;
        }
        Some(Pack {
            id: id.to_owned(),
            name: name.to_owned(),
            description: entry["description"].as_str().unwrap_or_default().to_owned(),
            link: link.to_owned(),
        })
    }).collect())
}

impl Render for Catalog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = AppState::global(cx);
        let mut content = v_flex().gap(px(14.)).child(
            div().text_size(px(24.)).child("Наши сборки"),
        );
        match &self.0 {
            CatalogState::Loading => content = content.child("Загрузка каталога…"),
            CatalogState::Unavailable => {
                content = content.child("Каталог недоступен. Установленные сборки доступны в библиотеке.")
                    .child(Button::new("catalog-retry").label("Повторить")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))));
            }
            CatalogState::Ready(packs) => {
                if packs.is_empty() {
                    content = content.child("В каталоге пока нет сборок.");
                }
                for pack in packs {
                    let id = pack.id.clone();
                    let link = pack.link.clone();
                    let installed = state.read(cx).packs.contains_key(&id);
                    let button_id = gpui::SharedString::from(format!("catalog-{id}"));
                    content = content.child(v_flex().gap(px(6.)).p(px(12.))
                        .child(div().text_size(px(17.)).child(pack.name.clone()))
                        .child(pack.description.clone())
                        .child(Button::new(button_id).primary()
                            .label(if installed { "Играть" } else { "Установить" })
                            .on_click(move |_, window, cx| {
                                let state = AppState::global(cx);
                                if state.read(cx).packs.contains_key(&id) {
                                    state.update(cx, |s, cx| s.navigate(Route::Instance(id.clone()), cx));
                                    launch_bar::launch(&id, window, cx);
                                } else {
                                    new_instance::install_catalog_link(&link, window, cx);
                                }
                            })));
                }
            }
        }
        div().id("blank-catalog").size_full().overflow_y_scroll().p(px(24.)).child(content)
    }
}
