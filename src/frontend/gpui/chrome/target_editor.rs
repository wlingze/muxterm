//! 目标配置窗口：runtime / transport 单选卡、路径、名字、SSH Host。
//!
//! 规则都在 `model/target_editor.rs`；这里只排布与命中。文本编辑沿用 composer
//! 的做法：`state.rs` 的按键路径维护焦点字段，不引入第二个输入控件。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};
use crate::frontend::gpui::model::target_editor::{TargetEditor, TargetEditorFocus};
use crate::frontend::gpui::state::GpuiApp;

impl GpuiApp {
    /// 目标配置浮层；没打开时返回 `None`。
    pub(crate) fn render_target_editor(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let editor = self.target_editor.as_ref()?;
        let theme = self.theme().clone();

        let runtimes: Vec<AnyElement> = TargetRuntime::ALL
            .iter()
            .map(|runtime| {
                let runtime = *runtime;
                let selected = editor.selection().runtime == runtime;
                div()
                    .id(SharedString::from(format!("target-runtime-{runtime:?}")))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .bg(if selected {
                        theme.active_bg
                    } else {
                        theme.chrome_bg
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.update_target_editor(|editor| editor.select_runtime(runtime), cx)
                        }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(runtime.label().to_string())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .child(SharedString::from(
                                TargetEditor::runtime_subtitle(runtime).to_string(),
                            )),
                    )
                    .into_any_element()
            })
            .collect();

        let ssh_selected = editor.ssh_field_visible();
        let transport_card = |transport: TargetTransport, label: &'static str| -> AnyElement {
            let selected = editor.selection().transport == transport;
            let transport_for_click = transport.clone();
            div()
                .id(SharedString::from(format!("target-transport-{label}")))
                .flex_1()
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(if selected {
                    theme.active_bg
                } else {
                    theme.chrome_bg
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.update_target_editor(
                            |editor| editor.select_transport(transport_for_click.clone()),
                            cx,
                        )
                    }),
                )
                .child(div().text_sm().child(SharedString::from(label)))
                .into_any_element()
        };

        let field =
            |focus: TargetEditorFocus, label: &'static str| -> AnyElement {
                let value = editor.focus_text(focus);
                let focused = self.target_editor_focus == focus;
                div()
                    .id(SharedString::from(format!("target-field-{label}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if focused { theme.accent } else { theme.border })
                    .bg(theme.chrome_bg)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_target_editor_focus(focus, cx)),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .text_xs()
                            .text_color(theme.muted)
                            .child(SharedString::from(label)),
                    )
                    .child(div().flex_1().text_sm().child(SharedString::from(
                        if value.is_empty() {
                            "—".to_string()
                        } else {
                            value
                        },
                    )))
                    .into_any_element()
            };

        let mut rows: Vec<AnyElement> = vec![
            field(TargetEditorFocus::Path, "Path"),
            field(TargetEditorFocus::Name, "Name"),
        ];
        if ssh_selected {
            rows.push(field(TargetEditorFocus::SshAlias, "SSH Host"));
        }
        let hint = editor.name_hint();
        let error = self.target_editor_error.clone().unwrap_or_default();

        Some(
            div()
                .id("target-editor-overlay")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .bg(gpui::rgba(0x00000099))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(560.0))
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg)
                        .child(div().text_base().child(SharedString::from(
                            if editor.is_editing() {
                                "Edit project"
                            } else {
                                "New project"
                            },
                        )))
                        .child(div().flex().flex_row().gap_2().children(runtimes))
                        .child(div().flex().flex_row().gap_2().children(vec![
                            transport_card(TargetTransport::Local, "local"),
                            transport_card(
                                TargetTransport::Ssh {
                                    name: editor.ssh_alias().to_string(),
                                },
                                "ssh",
                            ),
                        ]))
                        .children(rows)
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(SharedString::from(hint)),
                        )
                        .when(!error.is_empty(), |element| {
                            element.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child(SharedString::from(error)),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(SharedString::from("tab next field · ⏎ save · esc cancel")),
                        ),
                )
                .into_any_element(),
        )
    }
}
