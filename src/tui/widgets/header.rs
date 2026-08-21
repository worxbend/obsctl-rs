use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};
use rust_i18n::t;

use crate::tui::{anim, model::TuiModel, widgets::chrome};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let pulse = model.anim.pulse(30);
    let border = if model.advanced_ui {
        anim::blend(theme.border, theme.accent, pulse * 0.45)
    } else {
        theme.border
    };
    let title = if model.advanced_ui {
        anim::gradient_line(
            " ◈ OBSCTL // BROADCAST COMMAND CENTER ",
            theme.accent,
            theme.accent_alt,
            model.anim.frame,
            true,
        )
    } else {
        Line::styled(
            " OBSCTL // BROADCAST COMMAND CENTER ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    };
    let block = chrome::bordered(model, BorderType::Rounded, border, title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let brand_icon = model.symbol("⚡", "#");
    let first = Line::from(vec![
        Span::styled(
            format!("{brand_icon} obsctl"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  studio automation console",
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            if model.advanced_ui {
                "  •  "
            } else {
                "  |  "
            },
            Style::default().fg(theme.border),
        ),
        Span::styled(
            format!("frame {:06}", model.anim.frame),
            Style::default().fg(theme.info),
        ),
    ]);

    let daemon_text = if model.connected_to_daemon {
        t!("tui.header.daemon_connected").into_owned()
    } else {
        t!("tui.header.daemon_disconnected").into_owned()
    };
    let daemon = status_chip(
        &daemon_text,
        model.connected_to_daemon,
        model.rich_ui(),
        theme.success,
        theme.danger,
    );

    let obs_text = if model.obs_connected() {
        let version = model
            .snapshot()
            .and_then(|snapshot| snapshot.obs_studio_version.as_deref())
            .unwrap_or("?");
        t!("tui.header.obs_connected", version = version).into_owned()
    } else {
        t!("tui.header.obs_disconnected").into_owned()
    };
    let obs = status_chip(
        &obs_text,
        model.obs_connected(),
        model.rich_ui(),
        theme.success,
        theme.warning,
    );

    let mut status = vec![daemon, Span::raw("  "), obs];
    if let Some(scene) = model.current_scene() {
        status.extend([
            Span::styled(
                if model.advanced_ui { "  ◆ " } else { "  > " },
                Style::default().fg(theme.accent_alt),
            ),
            Span::styled(
                t!("tui.header.scene", scene = scene).into_owned(),
                Style::default().fg(theme.fg),
            ),
        ]);
    }
    if let Some(profile) = model.current_profile() {
        status.extend([
            Span::styled(
                if model.advanced_ui { "  ◇ " } else { "  - " },
                Style::default().fg(theme.info),
            ),
            Span::styled(
                t!("tui.header.profile", profile = profile).into_owned(),
                Style::default().fg(theme.muted),
            ),
        ]);
    }

    f.render_widget(Paragraph::new(vec![first, Line::from(status)]), inner);
}

fn status_chip(
    text: &str,
    active: bool,
    fancy: bool,
    active_color: ratatui::style::Color,
    inactive_color: ratatui::style::Color,
) -> Span<'static> {
    let color = if active { active_color } else { inactive_color };
    Span::styled(
        format!("{} {text}", chrome::status_dot(active, fancy)),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}
