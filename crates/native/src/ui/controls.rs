use nsh_base::{Input, SelectEntry, SelectItem, button, input, select, theme};
use nsh_core::prelude::*;

#[derive(Clone, Copy)]
pub enum Action {
    Primary,
    Secondary,
    Danger,
}

pub fn action(
    label: impl IntoReactive<SharedString>,
    variant: Action,
    disabled: impl Fn() -> bool + 'static,
    run: impl Fn() + 'static,
) -> impl IntoElement {
    button()
        .disabled(disabled)
        .on_press(run)
        .px_4()
        .py_2()
        .min_h_11()
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .border_1()
        .border_color(move || match variant {
            Action::Primary => theme().primary,
            Action::Secondary => theme().border,
            Action::Danger => theme().destructive,
        })
        .bg(move || match variant {
            Action::Primary => theme().primary,
            _ => theme().card,
        })
        .hover(|style| style.opacity(0.88))
        .active(|style| style.opacity(0.65))
        .focus(|style| style.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|style| style.opacity(0.45))
        .child(
            text(label)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || match variant {
                    Action::Primary => theme().primary_foreground,
                    Action::Secondary => theme().foreground,
                    Action::Danger => theme().destructive,
                }),
        )
}

pub fn field(value: Signal<String>, label: &'static str, placeholder: &'static str) -> Input {
    input()
        .label(label)
        .value(move || value.get())
        .on_change(move |text: &str| value.set(text.to_owned()))
        .placeholder(placeholder)
        .px_3()
        .py_2()
        .min_h_11()
        .border_1()
        .border_color(move || theme().input)
        .focus(|style| style.outline(px(2.0)).outline_color(move || theme().ring))
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .text_color(move || theme().foreground)
        .flex_1()
        .min_w_0()
}

pub fn heading(label: &'static str) -> impl IntoElement {
    text(label)
        .role(AccessibilityRole::Header)
        .accessibility_level(2)
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
}

pub fn choice(
    value: Signal<String>,
    label: &'static str,
    items: Memo<Vec<SelectItem>>,
) -> impl IntoElement {
    select()
        .accessibility_label(label)
        .disabled(move || items.with(Vec::is_empty))
        .items(move || items.get())
        .value(move || Some(SharedString::from(value.get())))
        .on_change(move |next: &Option<SharedString>| {
            if let Some(next) = next {
                value.set(next.to_string());
            }
        })
        .popup_style(|style| {
            style
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(move || theme().border)
                .bg(move || theme().popover)
                .shadow_md()
        })
        .item_class("rounded-md focus:bg-accent selected:font-medium disabled:opacity-50")
        .render_item(|item: &SelectEntry| {
            view().px_3().py_2().child(
                text(item.title.clone())
                    .text_sm()
                    .text_color(move || theme().popover_foreground),
            )
        })
        .focus(|style| style.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|style| style.opacity(0.45))
        .trigger(
            view()
                .min_h_11()
                .px_3()
                .py_2()
                .justify_center()
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || theme().input)
                .bg(move || theme().card)
                .child(
                    text(move || {
                        items.with(|items| {
                            items
                                .iter()
                                .find(|item| item.value.as_ref() == value.get().as_str())
                                .map(|item| item.title.to_string())
                                .unwrap_or_else(|| {
                                    format!("Display {} · find displays to choose", value.get())
                                })
                        })
                    })
                    .text_sm()
                    .text_color(move || theme().foreground),
                ),
        )
}

pub fn hint(label: &'static str) -> impl IntoElement {
    text(label)
        .text_sm()
        .text_color(move || theme().muted_foreground)
}

pub fn panel() -> nsh_core::element::View {
    view()
        .gap_4()
        .p_5()
        .rounded_lg()
        .border_1()
        .border_color(move || theme().border)
        .bg(move || theme().card)
}

pub fn row() -> nsh_core::element::View {
    view().flex_row().flex_wrap().items_center().gap_2()
}
