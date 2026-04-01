use crate::client::state::{App, AppMode};
use crate::protocol::{SyncDirection, SyncMode};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Always draw dashboard
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
        .split(size);

    draw_dashboard(f, app, chunks[0], chunks[1]);

    // Draw mode-specific popups
    match app.mode {
        AppMode::Dashboard => {}
        AppMode::LocalBrowser => draw_local_browser(f, app, size),
        AppMode::RemoteBrowser => draw_remote_browser(f, app, size),
        AppMode::PasswordInput => draw_password_input(f, app, size),
        AppMode::RemoteHostInput => draw_remote_host_input(f, app, size),
        AppMode::RemotePortInput => draw_remote_port_input(f, app, size),
        AppMode::SyncModeSelect => draw_sync_mode_select(f, app, size),
        AppMode::DryRunView => draw_dry_run_view(f, app, size),
        AppMode::HostSelect => draw_host_select(f, app, size),
        AppMode::CreateRemoteDir => draw_remote_mkdir_input(f, app, size),
        AppMode::LogView => draw_log_view(f, app, size),
        AppMode::CreateLocalDir => draw_local_mkdir_input(f, app, size),
    }
}

fn draw_dashboard(f: &mut Frame, app: &mut App, list_area: Rect, help_area: Rect) {
    // Split list area to make room for server status
    let dashboard_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(95), Constraint::Percentage(5)])
        .split(list_area);

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let color = match t.status.as_str() {
                "IDLE" => Color::Green,
                "ERROR" => Color::Red,
                "SYNCING..." => Color::Yellow,
                "PENDING..." => Color::Cyan,
                _ => Color::Blue,
            };

            let remote_display = format!("{}:{}", t.remote_host, t.remote_path);
            let mode_name = match t.sync_mode {
                SyncMode::Mirror => "Mirror",
                SyncMode::AddOnly => "AddOnly",
                SyncMode::SafeSync => "SafeSync",
                SyncMode::Update => "Update",
            };
            let compress_flag = if t.compress { "+Z" } else { "" };
            let style = if i == app.dashboard_selected_idx {
                Style::default().fg(color).bg(Color::DarkGray) // Highlight background
            } else {
                Style::default().fg(color)
            };

            ListItem::new(format!(
                "ID: {} | {} -> {}\n   [{}] Mode: {}{} | {}",
                t.id, t.source, remote_display, t.status, mode_name, compress_flag, t.last_log
            ))
            .style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title("Active Sync Tasks")
            .borders(Borders::ALL),
    );
    f.render_stateful_widget(list, dashboard_chunks[0], &mut app.dashboard_list_state);

    // Server status indicator
    let status_text = match app.server_status {
        Some(true) => ("Server: ONLINE", Color::Green),
        Some(false) => ("Server: OFFLINE", Color::Red),
        None => ("Server: ?", Color::Yellow),
    };
    let status_widget = Paragraph::new(status_text.0)
        .style(Style::default().fg(status_text.1))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(status_widget, dashboard_chunks[1]);

    let help = Paragraph::new(
        "Controls:\n[A] Add Task [Push] \t [P] Add Task [Pull] \t [D] Delete Task \t [R] Dry Run \n[S] Restart Task \t[L] View Logs \t[Ctrl+R] Restart Server \t[Q] Quit"
    )
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, help_area);
}

fn draw_local_browser(f: &mut Frame, app: &mut App, size: Rect) {
    let area = centered_rect(60, 60, size);
    f.render_widget(Clear, area);

    let browser_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let dirs: Vec<ListItem> = app
        .dir_entries
        .iter()
        .enumerate()
        .map(|(i, d): (usize, &String)| {
            let style = if i == app.selected_idx {
                Style::default().bg(Color::Blue)
            } else {
                Style::default()
            };
            ListItem::new(d.clone()).style(style)
        })
        .collect();

    let (title_text, instructions_text) = match app.pending_sync_direction {
        SyncDirection::Push => (
            "Select Source",
            "[Enter] Enter Dir  [Space] Select file/dir as Source [N] Create dir [Esc] Cancel",
        ),
        SyncDirection::Pull => (
            "Select Destination",
            "[Enter] Enter Dir  [Space] Select file/dir as Destination [N] Create dir [Esc] Back",
        ),
    };
    let title = format!("{}: {}", title_text, app.current_path);

    let b_block = Block::default().title(title).borders(Borders::ALL);
    f.render_widget(List::new(dirs).block(b_block), browser_chunks[0]);

    let instructions =
        Paragraph::new(instructions_text).block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, browser_chunks[1]);
}

fn draw_remote_browser(f: &mut Frame, app: &mut App, size: Rect) {
    let area = centered_rect(60, 60, size);
    f.render_widget(Clear, area);

    let browser_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let dirs: Vec<ListItem> = app
        .dir_entries
        .iter()
        .enumerate()
        .map(|(i, d): (usize, &String)| {
            let style = if i == app.selected_idx {
                Style::default().bg(Color::Blue)
            } else {
                Style::default()
            };
            ListItem::new(d.clone()).style(style)
        })
        .collect();

    let direction_text = match app.pending_sync_direction {
        SyncDirection::Push => "Select Remote Destination",
        SyncDirection::Pull => "Select Remote Source",
    };
    let title = format!(
        "{}: {} (Interface may freeze during SSH)",
        direction_text,
        if app.remote_current_path.is_empty() {
            format!("{}:/", app.pending_remote_host)
        } else {
            format!("{}:{}", app.pending_remote_host, app.remote_current_path)
        }
    );
    let instructions_text =
        "[Enter] Enter Dir  [Space] Select file/dir  [N] Create dir [Esc] Cancel";

    let b_block = Block::default().title(title).borders(Borders::ALL);
    let list = List::new(dirs).block(b_block);
    f.render_stateful_widget(list, browser_chunks[0], &mut app.browser_list_state);

    let instructions =
        Paragraph::new(instructions_text).block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, browser_chunks[1]);
}

fn draw_password_input(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(60, 25, size);
    f.render_widget(Clear, area);

    let pass_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(3)])
        .split(area);

    let display_text = if app.show_password {
        format!("{}|", app.input_password)
    } else {
        format!("{}|", "*".repeat(app.input_password.len()))
    };

    let input = Paragraph::new(format!(
        "SSH Password (leave empty for SSH keys):\n\n{}",
        display_text
    ))
    .block(
        Block::default()
            .title("Step 3: Enter SSH Password")
            .borders(Borders::ALL),
    );
    f.render_widget(input, pass_chunks[0]);

    let help = Paragraph::new("[Enter] Confirm  [Tab] Show/Hide  [Esc] Back")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, pass_chunks[1]);
}

fn draw_host_select(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(60, 60, size);
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let items: Vec<ListItem> = app
        .saved_hosts
        .iter()
        .enumerate()
        .map(|(i, h): (usize, &String)| {
            let style = if i == app.host_list_idx {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(h.as_str()).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title("Step 2: Select Remote Host")
            .borders(Borders::ALL),
    );
    f.render_widget(list, chunks[0]);

    let instructions = Paragraph::new("[Enter] Select  [A] Add  [E] Edit  [D] Delete")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[1]);
}

fn draw_remote_host_input(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(70, 25, size);
    f.render_widget(Clear, area);

    let input_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(3)])
        .split(area);

    // Show the input field with cursor
    let display_text = if app.input_cursor_pos <= app.input_remote_host.len() {
        if app.input_cursor_pos < app.input_remote_host.len() {
            format!(
                "{}|{}",
                &app.input_remote_host[..app.input_cursor_pos],
                &app.input_remote_host[app.input_cursor_pos..]
            )
        } else {
            format!("{}|", app.input_remote_host)
        }
    } else {
        format!("{}|", app.input_remote_host)
    };

    let input_block = Paragraph::new(format!(
        "Remote Host (user@hostname:port):\n\n{}",
        display_text
    ))
    .block(
        Block::default()
            .title("Step 2: Confirm/Edit Remote Host")
            .borders(Borders::ALL),
    );
    f.render_widget(input_block, input_chunks[0]);

    let instructions = Paragraph::new("[Enter] Continue  [Esc] Back  [←→] Move  [Home/End] Jump")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, input_chunks[1]);
}

fn draw_remote_port_input(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(70, 25, size);
    f.render_widget(Clear, area);

    let input_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(3)])
        .split(area);

    // Show the input field with cursor
    let display_text = if app.input_cursor_pos <= app.input_remote_port.len() {
        if app.input_cursor_pos < app.input_remote_port.len() {
            format!(
                "{}|{}",
                &app.input_remote_port[..app.input_cursor_pos],
                &app.input_remote_port[app.input_cursor_pos..]
            )
        } else {
            format!("{}|", app.input_remote_port)
        }
    } else {
        format!("{}|", app.input_remote_port)
    };

    let input_block = Paragraph::new(format!("SSH Port (default: 22):\n\n{}", display_text)).block(
        Block::default()
            .title("Step 2b: Enter SSH Port")
            .borders(Borders::ALL),
    );
    f.render_widget(input_block, input_chunks[0]);

    let instructions = Paragraph::new(
        "[Enter] Continue  [Esc] Back  [←→] Move  [Home/End] Jump  [0-9] Enter digits",
    )
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, input_chunks[1]);
}

fn draw_sync_mode_select(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(70, 50, size);
    f.render_widget(Clear, area);

    let sync_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(3)])
        .split(area);

    // Mode descriptions
    let modes = vec![
        (
            "Mirror",
            "Exact copy. Deletes files on remote if missing on local",
            Color::Yellow,
        ),
        (
            "Add Only",
            "Uploads new/changed files. Never deletes on remote",
            Color::Green,
        ),
        (
            "Safe Sync",
            "Mirrors but moves deleted files to .rsync-backup folder",
            Color::Cyan,
        ),
        (
            "Update",
            "Only overwrites if local file is newer",
            Color::Blue,
        ),
    ];

    let mut mode_items: Vec<ListItem> = vec![];
    for (idx, (name, desc, color)) in modes.iter().enumerate() {
        let prefix = if idx == app.sync_mode_selected_idx {
            "> "
        } else {
            "  "
        };
        let text = format!("{}{}", prefix, name);
        let item = if idx == app.sync_mode_selected_idx {
            ListItem::new(vec![
                ratatui::text::Line::from(text)
                    .style(Style::default().fg(*color).bg(Color::DarkGray)),
                ratatui::text::Line::from(format!("  {}", desc))
                    .style(Style::default().fg(Color::Gray)),
            ])
        } else {
            ListItem::new(vec![
                ratatui::text::Line::from(text).style(Style::default().fg(*color)),
                ratatui::text::Line::from(format!("  {}", desc))
                    .style(Style::default().fg(Color::DarkGray)),
            ])
        };
        mode_items.push(item);
    }

    // Add compression toggle
    let compress_text = if app.pending_compress {
        "[X] Enable Compression (-z)"
    } else {
        "[ ] Enable Compression (-z)"
    };
    mode_items.push(ListItem::new(""));
    mode_items.push(ListItem::new(compress_text).style(Style::default().fg(Color::White)));

    let mode_list = List::new(mode_items).block(
        Block::default()
            .title("Step 5: Select Sync Mode")
            .borders(Borders::ALL),
    );
    f.render_widget(mode_list, sync_chunks[0]);

    let instructions =
        Paragraph::new("[Enter] Continue  [↑↓] Navigate  [Space] Toggle Compress  [Esc] Back")
            .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, sync_chunks[1]);
}

fn draw_dry_run_view(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(80, 60, size);
    f.render_widget(Clear, area);

    let dry_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(3)])
        .split(area);

    // Create list items from dry run results
    let items: Vec<ListItem> = app
        .dry_run_results
        .iter()
        .skip(app.dry_run_scroll)
        .take(area.height as usize - 5) // Leave space for title and help
        .map(|s: &String| ListItem::new(s.as_str()))
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(format!("Dry Run Results: {}", app.dry_run_task_id))
            .borders(Borders::ALL),
    );
    f.render_widget(list, dry_chunks[0]);

    let help =
        Paragraph::new("[Esc] Close  [↑↓] Scroll").block(Block::default().borders(Borders::ALL));
    f.render_widget(help, dry_chunks[1]);
}

fn draw_remote_mkdir_input(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(60, 20, size); // Small popup
    f.render_widget(Clear, area);

    let input_block = Paragraph::new(format!("New Directory Name:\n\n{}|", app.input_new_dir))
        .block(
            Block::default()
                .title("Create Remote Directory")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        ); // Yellow border to stand out

    f.render_widget(input_block, area);
}

fn draw_local_mkdir_input(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(60, 20, size); // Small popup
    f.render_widget(Clear, area);

    let input_block = Paragraph::new(format!("New Directory Name:\n\n{}|", app.input_new_dir))
        .block(
            Block::default()
                .title("Create Local Directory")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        ); // Yellow border to stand out

    f.render_widget(input_block, area);
}

fn draw_log_view(f: &mut Frame, app: &App, size: Rect) {
    let area = centered_rect(90, 80, size);
    f.render_widget(Clear, area);

    let log_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(3)])
        .split(area);

    let title = format!("Logs: {}", app.view_log_task_id);
    let p = Paragraph::new(app.view_task_log.as_str())
        .block(Block::default().title(title.as_str()).borders(Borders::ALL))
        .scroll((app.view_log_scroll as u16, 0));

    f.render_widget(p, log_chunks[0]);

    let help = Paragraph::new("[Esc] Back  [↑↓] Scroll  [R] Refresh")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, log_chunks[1]);
}
