use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Instant,
};

use chrono::{DateTime, Duration, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
    Terminal,
};

const DB_PATH: &str = "./data/db.json";

#[derive(Serialize, Deserialize, Clone)]
struct Task {
    id: usize,
    project: String,
    created_at: DateTime<Utc>,
    running_since: Option<DateTime<Utc>>,
    times: Vec<TimeFrame>,
}

#[derive(Serialize, Deserialize, Clone)]
struct TimeFrame {
    id: usize,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
}

impl Task {
    fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    fn current_duration(&self) -> Duration {
        if let Some(running_since) = self.running_since {
            Utc::now() - running_since
        } else {
            Duration::zero()
        }
    }

    fn total_duration(&self) -> Duration {
        let past_duration = self.times.iter().fold(0, |acc, time_frame| {
            acc + (time_frame.end_time - time_frame.start_time).num_seconds()
        });

        Duration::seconds(past_duration + self.current_duration().num_seconds())
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("error reading the DB file: {0}")]
    ReadDBError(#[from] io::Error),
    #[error("error parsing the DB file: {0}")]
    ParseDBError(#[from] serde_json::Error),
}

enum Event<I> {
    Input(I),
    Tick,
}

struct App {
    state: State,
}

enum State {
    Projects,
    Help,
    CreateProject { input: String },
    DeleteProject,
}

enum Transitions {
    CreateNew,
    Delete,
    Escape,
    ShowHelp,
    InputCharacter(char),
}

impl App {
    fn transition(&mut self, transition: Transitions) {
        match (&self.state, transition) {
            (State::Projects, Transitions::CreateNew) => {
                self.state = State::CreateProject {
                    input: String::new(),
                }
            }
            (State::Projects, Transitions::Delete) => {
                self.state = State::DeleteProject;
            }
            (State::Projects, Transitions::ShowHelp) => {
                self.state = State::Help;
            }
            (State::Help, Transitions::Escape) => {
                self.state = State::Projects;
            }
            (State::CreateProject { input }, Transitions::InputCharacter(character)) => {
                self.state = State::CreateProject {
                    input: format!("{}{}", input, character),
                }
            }
            (State::CreateProject { input }, Transitions::Delete) => {
                self.state = State::CreateProject {
                    input: input[0..input.len() - 1].to_owned(),
                }
            }
            (State::CreateProject { input: _ }, Transitions::Escape) => {
                self.state = State::Projects;
            }
            (State::DeleteProject, Transitions::Escape) => {
                self.state = State::Projects;
            }
            (_, _) => {}
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode().expect("can run in raw mode");

    let (tx, rx) = mpsc::channel();
    let tick_rate = std::time::Duration::from_millis(200);
    thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| std::time::Duration::from_secs(0));

            if event::poll(timeout).expect("poll works") {
                if let CEvent::Key(key) = event::read().expect("can read events") {
                    tx.send(Event::Input(key)).expect("can send events");
                }
            }

            if last_tick.elapsed() >= tick_rate {
                if let Ok(_) = tx.send(Event::Tick) {
                    last_tick = Instant::now();
                }
            }
        }
    });

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App {
        state: State::Projects,
    };

    let mut task_list_state = TableState::default();
    task_list_state.select(Some(0));

    loop {
        terminal.draw(|rect| {
            let size = rect.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Min(2),
                        Constraint::Length(3),
                    ]
                    .as_ref(),
                )
                .split(size);

            let contextual_help = Paragraph::new("q: Quit | ?: Show help")
                .style(Style::default().fg(Color::LightCyan))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::White))
                        .title("Shortcuts")
                        .border_type(BorderType::Plain),
                );

            rect.render_widget(contextual_help, chunks[0]);

            let copyright = Paragraph::new("Time Tracking CLI")
                .style(Style::default().fg(Color::LightCyan))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::White))
                        .border_type(BorderType::Plain),
                );

            rect.render_widget(copyright, chunks[2]);

            match &app.state {
                State::Projects => {
                    let task_details = render_tasks();
                    rect.render_stateful_widget(task_details, chunks[1], &mut task_list_state);
                }
                State::Help => {
                    let help_popup = render_help_popup();
                    let area = centered_rect(40, 40, chunks[1]);

                    rect.render_widget(Clear, chunks[1]);
                    rect.render_widget(help_popup, area);
                }
                State::CreateProject { input } => {
                    let popup_input_field = render_create_popup(input);
                    let area = centered_rect(20, 20, chunks[1]);

                    rect.render_widget(Clear, chunks[1]);
                    rect.render_widget(
                        popup_input_field,
                        Rect {
                            x: area.x,
                            y: area.y,
                            height: 3,
                            width: area.width,
                        },
                    );
                }
                State::DeleteProject => {
                    let popup_input_field = render_delete_project_popup();
                    let area = centered_rect(40, 20, chunks[1]);

                    rect.render_widget(Clear, chunks[1]);
                    rect.render_widget(
                        popup_input_field,
                        Rect {
                            x: area.x,
                            y: area.y,
                            height: 3,
                            width: area.width,
                        },
                    )
                }
            }
        })?;

        match rx.recv()? {
            Event::Input(event) => match &app.state {
                State::Projects => match event.code {
                    KeyCode::Char('q') => {
                        disable_raw_mode()?;
                        execute!(
                            terminal.backend_mut(),
                            LeaveAlternateScreen,
                            DisableMouseCapture
                        )?;
                        terminal.show_cursor()?;
                        break;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(selected) = task_list_state.selected() {
                            let amount_tasks = read_db().expect("can fetch task list").len();
                            if selected >= amount_tasks - 1 {
                                task_list_state.select(Some(0));
                            } else {
                                task_list_state.select(Some(selected + 1));
                            }
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(selected) = task_list_state.selected() {
                            let amount_tasks = read_db().expect("can fetch task list").len();
                            if selected > 0 {
                                task_list_state.select(Some(selected - 1));
                            } else {
                                task_list_state.select(Some(amount_tasks - 1));
                            }
                        }
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        if let Some(selected) = task_list_state.selected() {
                            update_db(|tasks| {
                                let is_running = tasks[selected].is_running();

                                for task in tasks.iter_mut() {
                                    if let Some(running_since) = task.running_since {
                                        task.running_since = None;

                                        // TODO: Merge time frames that are within 15 minutes of
                                        // each other to help with fair rounding.

                                        let new_time_frame = TimeFrame {
                                            id: task.times.len(),
                                            start_time: running_since,
                                            end_time: Utc::now(),
                                        };

                                        task.times.push(new_time_frame);
                                    }
                                }

                                if !is_running {
                                    let mut selected_task =
                                        tasks.get_mut(selected).expect("exists");
                                    selected_task.running_since = Some(Utc::now());
                                }
                            })?;
                        }
                    }
                    KeyCode::Char('a') => {
                        app.transition(Transitions::CreateNew);
                    }
                    KeyCode::Char('d') => {
                        app.transition(Transitions::Delete);
                    }
                    KeyCode::Char('r') => {
                        let tasks = read_db()?;

                        let mut csv = String::new();

                        csv.push_str("Project,Duration\n");

                        for task in tasks {
                            csv.push_str(&format!(
                                "{},{}\n",
                                task.project,
                                format_duration_report(task.total_duration())
                            ));
                        }

                        fs::create_dir_all("./reports")?;
                        fs::write("./reports/latest_report.csv", csv)?;
                    }
                    KeyCode::Char('?') => {
                        app.transition(Transitions::ShowHelp);
                    }
                    KeyCode::Esc => {
                        app.transition(Transitions::Escape);
                    }
                    _ => {}
                },
                State::CreateProject { input } => match event.code {
                    KeyCode::Enter => {
                        update_db(|tasks| {
                            tasks.push(Task {
                                id: tasks.len(),
                                project: input.clone(),
                                times: vec![],
                                created_at: Utc::now(),
                                running_since: None,
                            });
                        })?;

                        app.transition(Transitions::Escape);
                    }
                    KeyCode::Char(c) => {
                        app.transition(Transitions::InputCharacter(c));
                    }
                    KeyCode::Backspace => {
                        app.transition(Transitions::Delete);
                    }
                    KeyCode::Esc => {
                        app.transition(Transitions::Escape);
                    }
                    _ => {}
                },
                State::DeleteProject => match event.code {
                    KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                        app.transition(Transitions::Escape);
                    }
                    KeyCode::Char('y') => {
                        update_db(|tasks| {
                            if let Some(selected) = task_list_state.selected() {
                                let _ = tasks.remove(selected);
                            }
                        })?;

                        app.transition(Transitions::Escape);
                    }
                    _ => {}
                },
                State::Help => match event.code {
                    KeyCode::Esc | KeyCode::Char('?' | 'q') => {
                        app.transition(Transitions::Escape);
                    }
                    _ => {}
                },
            },
            Event::Tick => {}
        }
    }

    Ok(())
}

fn render_help_popup<'a>() -> Table<'a> {
    Table::new(vec![
        Row::new(vec![
            Cell::from(Span::raw("a")),
            Cell::from(Span::raw("Add new project")),
        ]),
        Row::new(vec![
            Cell::from(Span::raw("d")),
            Cell::from(Span::raw("Delete selected project")),
        ]),
        Row::new(vec![
            Cell::from(Span::raw("<space>")),
            Cell::from(Span::raw("Start/stop project timer")),
        ]),
        Row::new(vec![
            Cell::from(Span::raw("r")),
            Cell::from(Span::raw("Generate a report")),
        ]),
        Row::new(vec![
            Cell::from(Span::raw("<esc>")),
            Cell::from(Span::raw("Close help")),
        ]),
    ])
    .header(Row::new(vec![
        Cell::from(Span::raw("Shortcut")),
        Cell::from(Span::raw("Description")),
    ]))
    .widths(&[Constraint::Percentage(20), Constraint::Percentage(80)])
    .block(Block::default().title("Help").borders(Borders::ALL))
}

fn render_create_popup<'a>(input: &'a str) -> Paragraph<'a> {
    Paragraph::new(input.as_ref()).block(
        Block::default()
            .title("New project name")
            .borders(Borders::ALL),
    )
}

fn render_delete_project_popup<'a>() -> Paragraph<'a> {
    Paragraph::new(Span::raw("y/n")).block(
        Block::default()
            .title("Confirm deletion")
            .borders(Borders::ALL),
    )
}

fn render_tasks<'a>() -> Table<'a> {
    let task_list = read_db().expect("can fetch task list");
    let rows: Vec<_> = task_list
        .iter()
        .map(|task| {
            Row::new(vec![
                Cell::from(Span::raw(task.project.clone())),
                Cell::from(Span::styled(
                    {
                        if task.is_running() {
                            format!("Running [{}]", format_duration(task.current_duration()))
                        } else {
                            "Not running".to_owned()
                        }
                    },
                    {
                        if task.is_running() {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default()
                        }
                    },
                )),
                Cell::from(Span::raw({
                    let duration = task.total_duration();

                    format_duration(duration)
                })),
            ])
        })
        .collect();

    let task_details = Table::new(rows)
        .header(Row::new(vec![
            Cell::from(Span::styled(
                "Project",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Cell::from(Span::styled(
                "Status",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Cell::from(Span::styled(
                "Total",
                Style::default().add_modifier(Modifier::BOLD),
            )),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White))
                .title("Details")
                .border_type(BorderType::Plain),
        )
        .widths(&[
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .highlight_style(Style::default().bg(Color::Rgb(60, 60, 60)));

    task_details
}

fn centered_rect(percent_x: u16, percent_y: u16, rect: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(rect);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.num_seconds();
    let seconds = total_secs % 60;
    let minutes = (total_secs / 60) % 60;
    let hours = (total_secs / 60) / 60;

    format!("{:0>2}:{:0>2}:{:0>2}", hours, minutes, seconds)
}

fn format_duration_report(duration: Duration) -> String {
    let total_minutes = (((duration.num_seconds() as f64) / 60.0 / 15.0).ceil() * 15.0) as i64;
    let minutes = total_minutes % 60;
    let hours = (total_minutes / 60) / 60;

    format!("{:0>2}:{:0>2}", hours, minutes)
}

fn read_db() -> Result<Vec<Task>, Error> {
    let mut db_content = String::new();
    match OpenOptions::new().read(true).open(DB_PATH) {
        Ok(mut file) => {
            file.read_to_string(&mut db_content)?;
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            db_content.push_str("[]");
        }
        Err(e) => {
            return Err(Error::ReadDBError(e));
        }
    };

    let parsed: Vec<Task> = serde_json::from_str(&db_content)?;

    Ok(parsed)
}

fn update_db(updater: impl Fn(&mut Vec<Task>) -> ()) -> Result<(), Error> {
    // Ensure path exists
    let db_path: PathBuf = DB_PATH.into();
    let db_dir = db_path.parent().unwrap_or("./".as_ref());
    fs::create_dir_all(db_dir)?;

    // Open file for reading and writing - ensure that file is created if does not exist
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .append(false)
        .open(DB_PATH)?;

    // Read and parse file
    let mut db_content = String::new();

    file.read_to_string(&mut db_content)?;

    if db_content.is_empty() {
        db_content.push_str("[]");
    }

    let mut parsed: Vec<Task> = serde_json::from_str(&db_content)?;

    // Update data
    updater(&mut parsed);

    // Write back to disk
    let serialized = &serde_json::to_vec(&parsed)?;
    fs::write(DB_PATH, serialized)?;

    Ok(())
}
