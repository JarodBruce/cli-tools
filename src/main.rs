use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use color_eyre::Result;
use crossterm::event;
use futures::StreamExt;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, LineGauge, Paragraph, Widget};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

type DownloadId = usize;

#[derive(Debug)]
enum Event {
    Input(event::KeyEvent),
    Tick,
    Resize,
    DownloadUpdate(DownloadId, u64, u64), // (id, downloaded, total)
    DownloadDone(DownloadId),
    DownloadError(DownloadId, String),
}

struct DownloadInProgress {
    id: DownloadId,
    name: String,
    started_at: Instant,
    downloaded: u64,
    total: u64,
}

impl DownloadInProgress {
    fn progress(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64) * 100.0
        }
    }
}

struct Downloads {
    in_progress: BTreeMap<DownloadId, DownloadInProgress>,
    completed: Vec<String>,
    errors: Vec<String>,
}

impl Downloads {
    fn new() -> Self {
        Self {
            in_progress: BTreeMap::new(),
            completed: Vec::new(),
            errors: Vec::new(),
        }
    }
}

async fn download_with_progress(
    id: DownloadId,
    url: &str,
    filename: &str,
    tx: mpsc::Sender<Event>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;
    let total_size = response.content_length().unwrap_or(0);
    
    let mut file = File::create(filename)?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        
        tx.send(Event::DownloadUpdate(id, downloaded, total_size))?;
        
        // 進捗更新の間隔を調整（より滑らかな表示のため）
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    tx.send(Event::DownloadDone(id))?;
    Ok(())
}

fn input_handling(tx: mpsc::Sender<Event>) {
    let tick_rate = Duration::from_millis(200);
    thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
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

fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    mut downloads: Downloads,
    rx: mpsc::Receiver<Event>,
) -> Result<()> {
    let mut redraw = true;
    loop {
        if redraw {
            terminal.draw(|frame| render(frame, &downloads))?;
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
            Event::DownloadUpdate(id, downloaded, total) => {
                if let Some(download) = downloads.in_progress.get_mut(&id) {
                    download.downloaded = downloaded;
                    download.total = total;
                }
                redraw = false;
            }
            Event::DownloadDone(id) => {
                if let Some(download) = downloads.in_progress.remove(&id) {
                    let duration = download.started_at.elapsed();
                    let size_mb = download.total as f64 / 1_048_576.0;
                    
                    terminal.insert_before(1, |buf| {
                        Paragraph::new(Line::from(vec![
                            Span::from("✓ ダウンロード完了: "),
                            Span::styled(
                                download.name.clone(),
                                Style::default().add_modifier(Modifier::BOLD).fg(Color::Green),
                            ),
                            Span::from(format!(
                                " ({:.2}MB, {}ms)",
                                size_mb,
                                duration.as_millis()
                            )),
                        ]))
                        .render(buf.area, buf);
                    })?;
                    
                    downloads.completed.push(download.name);
                    
                    if downloads.in_progress.is_empty() {
                        terminal.insert_before(1, |buf| {
                            Paragraph::new(Line::from(vec![
                                Span::styled("🎉 ", Style::default().fg(Color::Yellow)),
                                Span::styled("すべてのダウンロードが完了しました！", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                            ])).render(buf.area, buf);
                        })?;
                        break;
                    }
                }
            }
            Event::DownloadError(id, error) => {
                if let Some(download) = downloads.in_progress.remove(&id) {
                    terminal.insert_before(1, |buf| {
                        Paragraph::new(Line::from(vec![
                            Span::from("❌ エラー: "),
                            Span::styled(
                                download.name.clone(),
                                Style::default().add_modifier(Modifier::BOLD).fg(Color::Red),
                            ),
                            Span::from(format!(" - {}", error)),
                        ]))
                        .render(buf.area, buf);
                    })?;
                    downloads.errors.push(format!("{}: {}", download.name, error));
                }
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame, downloads: &Downloads) {
    let area = frame.area();

    let block = Block::new().title(Line::from("📥 ファイルダウンロード進捗").centered());
    frame.render_widget(block, area);

    let vertical = Layout::vertical([
        Constraint::Length(2), // 全体の進捗
        Constraint::Length(3), // ヘッダー
        Constraint::Min(4),    // ダウンロード詳細
    ]).margin(1);
    
    let areas = vertical.split(area);
    let progress_area = areas[0];
    let header_area = areas[1];
    let details_area = areas[2];

    // 全体の進捗
    let total_downloads = downloads.completed.len() + downloads.in_progress.len();
    let completed_downloads = downloads.completed.len();
    
    let progress = if total_downloads > 0 {
        completed_downloads as f64 / total_downloads as f64
    } else {
        0.0
    };
    
    let overall_progress = LineGauge::default()
        .filled_style(Style::default().fg(Color::Green))
        .label(format!("全体進捗 {}/{}", completed_downloads, total_downloads))
        .ratio(progress);
    frame.render_widget(overall_progress, progress_area);

    // ヘッダー情報
    let header_text = if downloads.in_progress.is_empty() {
        if downloads.completed.is_empty() {
            "ダウンロード待機中..."
        } else {
            "ダウンロード完了"
        }
    } else {
        "ダウンロード中..."
    };
    
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            header_text,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(header, header_area);

    // 個別ダウンロードの詳細
    let mut y_offset = 0;
    for (_, download) in downloads.in_progress.iter() {
        if y_offset >= details_area.height.saturating_sub(2) {
            break;
        }

        // ファイル名と統計情報
        let info_area = Rect {
            x: details_area.x,
            y: details_area.y + y_offset,
            width: details_area.width,
            height: 1,
        };
        
        let downloaded_mb = download.downloaded as f64 / 1_048_576.0;
        let total_mb = download.total as f64 / 1_048_576.0;
        let speed = if download.started_at.elapsed().as_secs() > 0 {
            downloaded_mb / download.started_at.elapsed().as_secs() as f64
        } else {
            0.0
        };
        
        let info_text = if download.total > 0 {
            format!(
                "📦 {} ({:.2}/{:.2}MB, {:.2}MB/s)",
                download.name, downloaded_mb, total_mb, speed
            )
        } else {
            format!("📦 {} ({:.2}MB, サイズ不明)", download.name, downloaded_mb)
        };
        
        let info = Paragraph::new(Line::from(vec![
            Span::styled(info_text, Style::default().fg(Color::White)),
        ]));
        frame.render_widget(info, info_area);

        // 進捗バー
        let gauge_area = Rect {
            x: details_area.x,
            y: details_area.y + y_offset + 1,
            width: details_area.width,
            height: 1,
        };

        let progress_ratio = download.progress() / 100.0;
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Yellow))
            .percent((progress_ratio * 100.0) as u16)
            .label(format!("{:.1}%", progress_ratio * 100.0));
        
        frame.render_widget(gauge, gauge_area);
        
        y_offset += 3;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(15),
    });

    let (tx, rx) = mpsc::channel();
    input_handling(tx.clone());
    
    let mut downloads = Downloads::new();
    
    // 複数のファイルをダウンロードするサンプル
    let download_tasks = vec![
        (0, "http://archive.ubuntu.com/ubuntu/pool/universe/b/bmon/bmon_4.0-6_amd64.deb", "bmon.deb"),
        (1, "https://httpbin.org/bytes/1024", "sample1.bin"),
        (2, "https://httpbin.org/bytes/2048", "sample2.bin"),
    ];

    // 全ダウンロードタスクを開始
    for (id, url, filename) in &download_tasks {
        let id = *id;
        downloads.in_progress.insert(
            id,
            DownloadInProgress {
                id,
                name: filename.to_string(),
                started_at: Instant::now(),
                downloaded: 0,
                total: 0,
            },
        );

        let tx_clone = tx.clone();
        let url_owned = url.to_string();
        let filename_owned = filename.to_string();
        
        tokio::spawn(async move {
            if let Err(e) = download_with_progress(id, &url_owned, &filename_owned, tx_clone.clone()).await {
                let _ = tx_clone.send(Event::DownloadError(id, e.to_string()));
            }
        });
    }

    // 実行前にターミナルを閉じる
    ratatui::restore();

    // ダウンロードが完了したら、.deb ファイルをインストールする
    println!("すべてのダウンロードが完了しました。");
    let deb_files: Vec<&str> = download_tasks
        .iter()
        .filter_map(|(_, _, filename)| {
            if filename.ends_with(".deb") {
                Some(*filename)
            } else {
                None
            }
        })
        .collect();

    if !deb_files.is_empty() {
        println!(".deb ファイルのインストールを試みます...");
        let mut args = vec!["-i"];
        args.extend(deb_files);

        let status = std::process::Command::new("sudo")
            .arg("dpkg")
            .args(&args)
            .status()?;

        if status.success() {
            println!("インストールが正常に完了しました。");
        } else {
            eprintln!("インストールに失敗しました。終了コード: {:?}", status.code());
        }
    }

    let app_result = run(&mut terminal, downloads, rx);
    ratatui::restore();
    
    app_result
}