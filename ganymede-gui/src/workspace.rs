//! In-memory tabs and launcher; server-backed terminals and persistence come later.
use crate::{
    code_cell::CodeCell,
    icon::Icon,
    notebook::Notebook,
    theme::{ActiveTheme, Theme},
};
use gpui::{
    App, Context, Entity, Focusable, FontWeight, ScrollHandle, SharedString, Window, div,
    prelude::*, px,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Notebook,
    Text,
    Markdown,
    Python,
    Terminal,
    Console,
}
impl DocumentKind {
    fn icon(self) -> Icon {
        match self {
            Self::Notebook => Icon::Notebook,
            Self::Terminal | Self::Console => Icon::Terminal,
            Self::Python => Icon::FileCode,
            _ => Icon::FileText,
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Notebook => "ipynb",
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Python => "py",
            Self::Terminal | Self::Console => "",
        }
    }
}
enum Content {
    Notebook(Entity<Notebook>),
    Text(Entity<CodeCell>),
    Terminal,
    Console,
}
struct Document {
    id: u64,
    name: SharedString,
    kind: DocumentKind,
    content: Content,
}
// Zero is reserved for the one launcher tab.
const LAUNCHER: u64 = 0;
#[derive(Debug)]
struct Tabs {
    open: Vec<u64>,
    active: u64,
}
impl Default for Tabs {
    fn default() -> Self {
        Self {
            open: vec![LAUNCHER],
            active: LAUNCHER,
        }
    }
}
impl Tabs {
    fn activate(&mut self, id: u64) {
        if !self.open.contains(&id) {
            self.open.push(id);
        }
        self.active = id;
    }
    fn launch(&mut self, id: u64) {
        if let Some(index) = self.open.iter().position(|tab| *tab == LAUNCHER) {
            self.open[index] = id;
        }
        self.activate(id);
    }
    fn close(&mut self, id: u64) {
        let Some(index) = self.open.iter().position(|tab| *tab == id) else {
            return;
        };
        self.open.remove(index);
        if self.open.is_empty() {
            self.activate(LAUNCHER);
        } else if self.active == id {
            self.active = self.open[index.min(self.open.len() - 1)];
        }
    }
}
pub struct Workspace {
    documents: Vec<Document>,
    tabs: Tabs,
    tab_scroll: ScrollHandle,
    next_id: u64,
}
impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let mut this = Self {
            documents: vec![],
            tabs: Tabs::default(),
            tab_scroll: ScrollHandle::new(),
            next_id: 0,
        };
        this.create(DocumentKind::Notebook, window, cx);
        this
    }
    fn create(&mut self, kind: DocumentKind, window: &mut Window, cx: &mut Context<Self>) {
        self.next_id += 1;
        let number = self.documents.iter().filter(|doc| doc.kind == kind).count();
        let name = if kind == DocumentKind::Console {
            format!("Console {}", number + 1)
        } else if kind == DocumentKind::Terminal {
            format!("Terminal {}", number + 1)
        } else if number == 0 {
            format!("Untitled.{}", kind.extension())
        } else {
            format!("Untitled-{}.{}", number + 1, kind.extension())
        };
        let content = match kind {
            DocumentKind::Notebook => {
                let notebook = cx.new(|cx| Notebook::new(window, cx));
                cx.observe(&notebook, |_, _, cx| cx.notify()).detach();
                Content::Notebook(notebook)
            }
            DocumentKind::Terminal => Content::Terminal,
            DocumentKind::Console => Content::Console,
            _ => {
                let editor = cx.new(|cx| CodeCell::new_document(window, cx));
                cx.subscribe(&editor, |_, _, event, cx| {
                    if matches!(event, crate::code_cell::CellEvent::Changed) {
                        cx.notify();
                    }
                })
                .detach();
                editor.read(cx).focus_handle(cx).focus(window, cx);
                Content::Text(editor)
            }
        };
        self.documents.push(Document {
            id: self.next_id,
            name: name.into(),
            kind,
            content,
        });
        self.tabs.launch(self.next_id);
        self.activate(self.next_id, window, cx);
    }
    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate(id);
        self.tab_scroll
            .scroll_to_item(self.tabs.open.iter().position(|tab| *tab == id).unwrap());
        if let Some(doc) = self.documents.iter().find(|doc| doc.id == id) {
            match &doc.content {
                Content::Notebook(notebook) => {
                    notebook.update(cx, |notebook, cx| notebook.focus_selected(window, cx))
                }
                Content::Text(editor) => editor.read(cx).focus_handle(cx).focus(window, cx),
                _ => window.blur(),
            }
        } else {
            window.blur();
        }
        cx.notify();
    }
    pub fn open_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(LAUNCHER, window, cx);
    }
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(self.tabs.active, window, cx);
    }
    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.close(id);
        self.activate(self.tabs.active, window, cx);
        // Drafts are retained in Files even after closing their tabs.
    }
    pub fn header(entity: &Entity<Self>, cx: &App) -> gpui::Div {
        let this = entity.read(cx);
        let t = cx.theme();
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .ml_3()
            .mr_2()
            .child(
                div()
                    .id("workspace-tabs")
                    .flex()
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .track_scroll(&this.tab_scroll)
                    .children(this.tabs.open.iter().map(|id| {
                        let id = *id;
                        let doc = this.documents.iter().find(|doc| doc.id == id);
                        let selected = this.tabs.active == id;
                        let name = doc
                            .map(|doc| doc.name.clone())
                            .unwrap_or_else(|| "Launcher".into());
                        let icon = doc.map(|doc| doc.kind.icon()).unwrap_or(Icon::SquaresFour);
                        let (dirty, running) = match doc.map(|doc| &doc.content) {
                            Some(Content::Notebook(notebook)) => (
                                notebook.read(cx).is_dirty(cx),
                                notebook.read(cx).running_spinner(cx),
                            ),
                            Some(Content::Text(editor)) => {
                                (!editor.read(cx).text().is_empty(), None)
                            }
                            _ => (false, None),
                        };
                        let fg = if selected {
                            t.app.fg
                        } else {
                            t.sidebar.muted_fg
                        };
                        let select = entity.clone();
                        let select_key = entity.clone();
                        let close = entity.clone();
                        let close_key = entity.clone();
                        let leading = if let Some(glyph) = running {
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_family(t.font.mono.clone())
                                .text_size(t.font.sm)
                                .text_color(fg)
                                .child(glyph.to_string())
                                .into_any_element()
                        } else {
                            icon.svg().size(px(14.)).text_color(fg).into_any_element()
                        };
                        div()
                            .id(("document-tab", id))
                            .tab_index(0)
                            .h(px(28.))
                            .w(px(180.))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded(t.radius.md())
                            .text_size(t.font.xs)
                            .cursor_pointer()
                            .when(selected, |s| {
                                // Inset shadows use this element's rounded bounds; clipping
                                // also keeps tab content inside the same silhouette.
                                s.overflow_hidden()
                                    .bg(t.app.bg)
                                    .text_color(t.app.fg)
                                    .shadow(vec![
                                        t.body_shadow.clone(),
                                        t.active_tab_edges[0].clone(),
                                        t.active_tab_edges[1].clone(),
                                    ])
                            })
                            .when(!selected, |s| {
                                s.text_color(t.sidebar.muted_fg)
                                    .hover(|s| s.bg(t.rail_hover_bg))
                            })
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                select.update(cx, |this, cx| this.activate(id, window, cx));
                            })
                            .on_key_down(move |event, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    select_key.update(cx, |this, cx| this.activate(id, window, cx));
                                    cx.stop_propagation();
                                }
                            })
                            .child(div().size(px(14.)).flex_shrink_0().child(leading))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .child(
                                div()
                                    .id(("close-tab", id))
                                    .group("tab-close")
                                    .aria_label(if dirty {
                                        "Close tab — unsaved changes"
                                    } else {
                                        "Close tab"
                                    })
                                    .tab_index(0)
                                    .size(px(18.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(t.radius.sm())
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.rail_hover_bg))
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        close.update(cx, |this, cx| this.close(id, window, cx));
                                    })
                                    .on_key_down(move |event, window, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            close_key
                                                .update(cx, |this, cx| this.close(id, window, cx));
                                            cx.stop_propagation();
                                        }
                                    })
                                    .child(if dirty {
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(fg)
                                            .text_color(t.app.bg)
                                            .into_any_element()
                                    } else {
                                        Icon::X
                                            .svg()
                                            .size(px(12.))
                                            .text_color(fg)
                                            .into_any_element()
                                    }),
                            )
                    })),
            )
            .child({
                let click = entity.clone();
                let key = entity.clone();
                icon_button("new-tab", "New tab", Icon::Plus, cx)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        click.update(cx, |this, cx| this.open_launcher(window, cx));
                    })
                    .on_key_down(move |event, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            key.update(cx, |this, cx| this.open_launcher(window, cx));
                            cx.stop_propagation();
                        }
                    })
            })
    }
    pub fn files(entity: &Entity<Self>, cx: &App, notebooks: bool) -> gpui::Div {
        let t = cx.theme();
        let this = entity.read(cx);
        div().flex().flex_col().gap(px(2.)).children(
            this.documents
                .iter()
                .filter(|doc| {
                    !matches!(doc.kind, DocumentKind::Terminal | DocumentKind::Console)
                        && (doc.kind == DocumentKind::Notebook) == notebooks
                })
                .map(|doc| {
                    let selected = this.tabs.active == doc.id;
                    let id = doc.id;
                    let entity = entity.clone();
                    div()
                        .id(("workspace-file", id))
                        .h(px(24.))
                        .pl(px(if notebooks { 16. } else { 4. }))
                        .pr_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded(t.radius.md())
                        .cursor_pointer()
                        .text_size(t.font.xs)
                        .when(selected, |s| {
                            s.bg(t.active_file_bg).text_color(t.active_file_fg)
                        })
                        .when(!selected, |s| {
                            s.text_color(t.sidebar_content.fg)
                                .hover(|s| s.bg(t.sidebar_content.accent))
                        })
                        .child(doc.kind.icon().svg().size(px(14.)).text_color(if selected {
                            t.active_file_fg
                        } else {
                            t.sidebar_content.fg
                        }))
                        .child(div().truncate().child(doc.name.clone()))
                        .on_click(move |_, window, cx| {
                            entity.update(cx, |this, cx| this.activate(id, window, cx))
                        })
                }),
        )
    }
    fn launcher(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = cx.theme();
        let groups = [
            (
                "Notebook",
                vec![(DocumentKind::Notebook, "Python 3", "ipykernel")],
            ),
            (
                "Console",
                vec![(DocumentKind::Console, "Python 3", "ipykernel")],
            ),
            (
                "Other",
                vec![
                    (DocumentKind::Terminal, "Terminal", ""),
                    (DocumentKind::Text, "Text file", ".txt"),
                    (DocumentKind::Markdown, "Markdown file", ".md"),
                    (DocumentKind::Python, "Python file", ".py"),
                ],
            ),
        ];
        div()
            .id("launcher")
            .size_full()
            .overflow_y_scroll()
            .p_5()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .children(groups.into_iter().map(|(heading, items)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(t.font.sm)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(heading),
                            )
                            .child(div().grid().grid_cols(2).gap_2().children(
                                items.into_iter().map(|(kind, label, detail)| {
                                    div()
                                        .id(SharedString::from(format!("{heading}-{label}")))
                                        .tab_index(0)
                                        .h(px(40.))
                                        .px_3()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(t.radius.lg())
                                        .bg(t.sidebar_content.bg)
                                        .text_color(t.sidebar_content.fg)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.rail_hover_bg))
                                        .focus(|s| s.border_1().border_color(t.app.ring))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.create(kind, window, cx)
                                        }))
                                        .on_key_down(cx.listener(
                                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) {
                                                    this.create(kind, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            },
                                        ))
                                        .child(
                                            kind.icon()
                                                .svg()
                                                .size(px(16.))
                                                .flex_shrink_0()
                                                .text_color(t.sidebar_content.muted_fg),
                                        )
                                        .child(div().text_size(t.font.sm).truncate().child(label))
                                        .child(div().flex_1())
                                        .when(!detail.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .text_size(t.font.xs)
                                                    .text_color(t.sidebar_content.muted_fg)
                                                    .child(detail),
                                            )
                                        })
                                }),
                            ))
                    })),
            )
    }
}
impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let content = self.documents.iter().find(|doc| doc.id == self.tabs.active);
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(match content {
                Some(Document {
                    content: Content::Notebook(notebook),
                    ..
                }) => notebook.clone().into_any_element(),
                Some(Document {
                    content: Content::Text(editor),
                    ..
                }) => div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .child(editor.clone())
                    .into_any_element(),
                Some(Document {
                    content: Content::Terminal | Content::Console,
                    ..
                }) => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Icon::Terminal
                            .svg()
                            .size(px(24.))
                            .text_color(t.app.muted_fg),
                    )
                    .child(div().text_size(t.font.sm).child("No server connected"))
                    .child(
                        div()
                            .text_size(t.font.xs)
                            .text_color(t.app.muted_fg)
                            .child("Connect to a Jupyter server to start a terminal."),
                    )
                    .into_any_element(),
                None => self.launcher(cx).into_any_element(),
            })
    }
}
pub fn icon_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    icon: Icon,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let t = cx.theme();
    div()
        .id(id)
        .aria_label(label)
        .tab_index(0)
        .size(px(26.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius.sm())
        .text_color(t.app.fg)
        .cursor_pointer()
        .hover(|s| s.bg(t.rail_hover_bg))
        .active(|s| s.bg(t.sidebar.accent))
        .focus(|s| s.border_1().border_color(t.app.ring))
        .tooltip(move |_, cx| cx.new(|_| Tip(label)).into())
        .child(icon.svg().size(px(14.)).text_color(t.app.fg))
}
struct Tip(&'static str);
impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(t.radius.sm())
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .border_1()
            .border_color(t.popover.border)
            .text_size(t.font.xs)
            .child(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_is_singleton_and_remains_open_when_switching_tabs() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.activate(LAUNCHER);
        assert_eq!(tabs.open, vec![LAUNCHER, 1]);
        tabs.activate(1);
        assert!(tabs.open.contains(&LAUNCHER));
    }
    #[test]
    fn launching_replaces_launcher_in_place_and_selects_new_tab() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.launch(2);
        assert_eq!(tabs.open, vec![2, 1]);
        assert_eq!(tabs.active, 2);
        tabs.close(1);
        tabs.close(2);
        assert_eq!(tabs.open, vec![LAUNCHER]);
    }
    #[test]
    fn closing_last_document_creates_launcher() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.close(LAUNCHER);
        tabs.close(1);
        assert_eq!(tabs.open, vec![LAUNCHER]);
        assert_eq!(tabs.active, LAUNCHER);
    }
    #[test]
    fn closing_only_launcher_recreates_it() {
        let mut tabs = Tabs::default();
        tabs.close(LAUNCHER);
        assert_eq!(tabs.open, vec![LAUNCHER]);
        assert_eq!(tabs.active, LAUNCHER);
    }
    #[test]
    fn closing_launcher_selects_another_tab_without_recreating_it() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.close(LAUNCHER);
        assert_eq!(tabs.open, vec![1]);
        assert_eq!(tabs.active, 1);
    }
    #[test]
    fn closing_background_tab_keeps_selection() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(2);
        tabs.close(1);
        assert_eq!(tabs.active, 2);
    }
}
