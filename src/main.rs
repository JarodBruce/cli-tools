/// A Ratatui example that visualizes package installation progress.
///
/// It shows the status of installing packages with apt-get, displaying
/// progress bars for each installation step.
///
/// Based on the Ratatui download progress example.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use color_eyre::Result;
use crossterm::event;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, LineGauge, List, ListItem, Paragraph, Widget};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

fn main() -> Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(8),
    });

    let (tx, rx) = mpsc::channel();
    input_handling(tx.clone());
    let workers = workers(tx);
    let mut installations = packages();

    for w in &workers {
        let p = installations.next(w.id).unwrap();
        w.tx.send(p).unwrap();
    }

    let app_result = run(&mut terminal, workers, installations, rx);

    ratatui::restore();

    app_result
}

const NUM_PACKAGES: usize = 5;

type PackageId = usize;
type WorkerId = usize;
enum Event {
    Input(event::KeyEvent),
    Tick,
    Resize,
    InstallUpdate(WorkerId, PackageId, f64),
    InstallDone(WorkerId, PackageId),
}
struct Installations {
    pending: VecDeque<Package>,
    in_progress: BTreeMap<WorkerId, PackageInProgress>,
}

impl Installations {
    fn next(&mut self, worker_id: WorkerId) -> Option<Package> {
        match self.pending.pop_front() {
            Some(p) => {
                self.in_progress.insert(
                    worker_id,
                    PackageInProgress {
                        id: p.id,
                        name: p.name.clone(),
                        started_at: Instant::now(),
                        progress: 0.0,
                    },
                );
                Some(p)
            }
            None => None,
        }
    }
}
struct PackageInProgress {
    id: PackageId,
    name: String,
    started_at: Instant,
    progress: f64,
}
struct Package {
    id: PackageId,
    name: String,
    size: usize,
}
struct Worker {
    id: WorkerId,
    tx: mpsc::Sender<Package>,
}

fn input_handling(tx: mpsc::Sender<Event>) {
    let tick_rate = Duration::from_millis(200);
    thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            // poll for tick rate duration, if no events, sent tick event.
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout).unwrap() {
                match event::read().unwrap() {
                    event::Event::Key(key) => tx.send(Event::Input(key)).unwrap(),
                    event::Event::Resize(_, _) => tx.send(Event::Resize).unwrap(),
                    _ => {}
                }
            }
            if last_tick.elapsed() >= tick_rate {
                tx.send(Event::Tick).unwrap();
                last_tick = Instant::now();
            }
        }
    });
}

#[expect(clippy::cast_precision_loss, clippy::needless_pass_by_value)]
fn workers(tx: mpsc::Sender<Event>) -> Vec<Worker> {
    (0..2)
        .map(|id| {
            let (worker_tx, worker_rx) = mpsc::channel::<Package>();
            let tx = tx.clone();
            thread::spawn(move || {
                while let Ok(package) = worker_rx.recv() {
                    let mut remaining = package.size;
                    while remaining > 0 {
                        let wait = (remaining as u64).min(10);
                        thread::sleep(Duration::from_millis(wait * 15));
                        remaining = remaining.saturating_sub(10);
                        let progress = (package.size - remaining) * 100 / package.size;
                        tx.send(Event::InstallUpdate(id, package.id, progress as f64))
                            .unwrap();
                    }
                    tx.send(Event::InstallDone(id, package.id)).unwrap();
                }
            });
            Worker { id, tx: worker_tx }
        })
        .collect()
}

fn packages() -> Installations {
    let packages = vec![
        ("git", 150),
        ("git-man", 80),
        ("liberror-perl", 40),
        ("git-core", 100),
        ("ca-certificates", 60),
    ];
    
    let pending = packages
        .into_iter()
        .enumerate()
        .map(|(id, (name, size))| Package {
            id,
            name: name.to_string(),
            size,
        })
        .collect();
    
    Installations {
        pending,
        in_progress: BTreeMap::new(),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    workers: Vec<Worker>,
    mut installations: Installations,
    rx: mpsc::Receiver<Event>,
) -> Result<()> {
    let mut redraw = true;
    loop {
        if redraw {
            terminal.draw(|frame| render(frame, &installations))?;
        }
        redraw = true;

        match rx.recv()? {
            Event::Input(event) => {
                if event.code == event::KeyCode::Char('q') {
                    break;
                }
            }
            Event::Resize => {
                terminal.autoresize()?;
            }
            Event::Tick => {}
            Event::InstallUpdate(worker_id, _package_id, progress) => {
                let package = installations.in_progress.get_mut(&worker_id).unwrap();
                package.progress = progress;
                redraw = false;
            }
            Event::InstallDone(worker_id, _package_id) => {
                let package = installations.in_progress.remove(&worker_id).unwrap();
                terminal.insert_before(1, |buf| {
                    Paragraph::new(Line::from(vec![
                        Span::from("✓ Installed "),
                        Span::styled(
                            package.name.clone(),
                            Style::default().add_modifier(Modifier::BOLD).fg(Color::Green),
                        ),
                        Span::from(format!(
                            " in {}ms",
                            package.started_at.elapsed().as_millis()
                        )),
                    ]))
                    .render(buf.area, buf);
                })?;
                match installations.next(worker_id) {
                    Some(p) => workers[worker_id].tx.send(p).unwrap(),
                    None => {
                        if installations.in_progress.is_empty() {
                            terminal.insert_before(1, |buf| {
                                Paragraph::new(Line::from(vec![
                                    Span::styled("🎉 ", Style::default().fg(Color::Yellow)),
                                    Span::styled("Installation complete!", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                                    Span::from(" git is now available."),
                                ])).render(buf.area, buf);
                            })?;
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame, installations: &Installations) {
    let area = frame.area();

    let block = Block::new().title(Line::from("📦 Package Installation Progress").centered());
    frame.render_widget(block, area);

    let vertical = Layout::vertical([Constraint::Length(2), Constraint::Length(4)]).margin(1);
    let horizontal = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]);
    let areas = vertical.split(area);
    let progress_area = areas[0];
    let main = areas[1];
    let main_areas = horizontal.split(main);
    let list_area = main_areas[0];
    let gauge_area = main_areas[1];

    // total progress
    let done = NUM_PACKAGES - installations.pending.len() - installations.in_progress.len();
    #[expect(clippy::cast_precision_loss)]
    let progress = LineGauge::default()
        .filled_style(Style::default().fg(Color::Green))
        .label(format!("Installing packages {done}/{NUM_PACKAGES}"))
        .ratio(done as f64 / NUM_PACKAGES as f64);
    frame.render_widget(progress, progress_area);

    // in progress installations
    let items: Vec<ListItem> = installations
        .in_progress
        .values()
        .map(|package| {
            ListItem::new(Line::from(vec![
                Span::styled("📦 ", Style::default().fg(Color::Blue)),
                Span::styled(
                    package.name.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " ({}ms)",
                    package.started_at.elapsed().as_millis()
                )),
            ]))
        })
        .collect();
    let list = List::new(items);
    frame.render_widget(list, list_area);

    #[expect(clippy::cast_possible_truncation)]
    for (i, (_, package)) in installations.in_progress.iter().enumerate() {
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Yellow))
            .ratio(package.progress / 100.0);
        if gauge_area.top().saturating_add(i as u16) > area.bottom() {
            continue;
        }
        frame.render_widget(
            gauge,
            Rect {
                x: gauge_area.left(),
                y: gauge_area.top().saturating_add(i as u16),
                width: gauge_area.width,
                height: 1,
            },
        );
    }
}