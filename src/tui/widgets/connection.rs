use ratatui::{
    Frame,
    layout::Rect,
    style::Modifier,
    style::Style,
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};
use rust_i18n::t;

use crate::service::systemd_user_service::SYSTEMCTL_ENABLE_HINT;
use crate::tui::{model::TuiModel, widgets::chrome};

pub fn render_unavailable(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let could_not_connect = t!("tui.connection.could_not_connect");
    let err = model.last_result.as_deref().unwrap_or(&could_not_connect);

    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}  ", model.symbol("⚠", "!")),
                Style::default().fg(theme.danger),
            ),
            Span::styled(
                t!("tui.connection.daemon_not_running").into_owned(),
                Style::default()
                    .fg(theme.danger)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::styled(err.to_string(), Style::default().fg(theme.warning)),
        Line::raw(""),
        Line::raw(t!("tui.connection.start_daemon_with").into_owned()),
        Line::styled(
            "  obsctl server --headless",
            Style::default().fg(theme.info),
        ),
        Line::raw(t!("tui.connection.or_install_service").into_owned()),
        Line::styled("  obsctl service install", Style::default().fg(theme.info)),
        Line::styled(
            format!("  {SYSTEMCTL_ENABLE_HINT}"),
            Style::default().fg(theme.info),
        ),
        Line::raw(""),
        Line::styled(
            t!("tui.connection.press_r_to_retry").into_owned(),
            Style::default().fg(theme.muted),
        ),
    ];

    let unavailable_title = chrome::typographic(model, &t!("tui.connection.title_unavailable"));
    let title = chrome::heading(model, unavailable_title, theme.danger, theme.warning);
    let border_color = chrome::breathing_border(model, theme.danger, theme.warning, 1.0);
    let block = chrome::bordered(model, BorderType::Double, border_color, title);
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let lines: Vec<Line> = if !model.connected_to_daemon {
        vec![
            Line::styled(
                t!("tui.connection.not_connected").into_owned(),
                Style::default().fg(theme.danger),
            ),
            Line::raw(t!("tui.connection.press_r_to_retry").into_owned()),
        ]
    } else if let Some(snap) = model.snapshot() {
        let status = if snap.connected {
            t!("tui.connection.obs_connected")
        } else {
            t!("tui.connection.obs_disconnected")
        };
        let mut v = vec![Line::from(status.into_owned())];
        if let Some(e) = &snap.last_error {
            v.push(Line::styled(
                t!("tui.connection.last_error", error = e).into_owned(),
                Style::default().fg(theme.warning),
            ));
        }
        v
    } else {
        vec![Line::raw(
            t!("tui.connection.waiting_for_state").into_owned(),
        )]
    };

    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        theme.border,
        t!("tui.connection.title").into_owned(),
    );
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };
    f.render_widget(Paragraph::new(lines), inner);
}
