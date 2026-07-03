//! The terminal UI: tab state, rendering, and the async event loop. Manages
//! raw mode / the alternate screen (restored on panic) and drives shutdown.

mod app;
mod color;
mod ui;

pub use app::App;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::logger::Logger;
use crate::runner::Runner;
use crate::service::Service;
use crate::theme::Theme;

/// Min interval between redraws so a noisy service can't spin the renderer.
const REDRAW_COALESCE: Duration = Duration::from_millis(16);

/// Restores the terminal to cooked mode. Idempotent; safe to call from a
/// signal handler before a forced exit, or after a panic.
pub fn restore_terminal() {
    ratatui::restore();
}

/// Runs the supervisor in the background and drives the UI until the user quits,
/// then ensures shutdown and returns the exit code. Restores the terminal on return/panic.
pub async fn run(
    services: &Arc<[Service]>,
    runner: &Runner,
    logger: &Logger,
    theme: Theme,
    force_delay: Duration,
) -> io::Result<i32> {
    let system = Arc::clone(logger.system().store());
    let mut app = App::new(Arc::clone(services), system, logger.pad_width(), theme);

    // The supervisor runs concurrently; the UI learns the exit code via fin_rx.
    let (fin_tx, fin_rx) = oneshot::channel();
    let run_handle = {
        let runner = runner.clone();
        tokio::spawn(async move {
            let code = runner.run().await;
            let _ = fin_tx.send(code);
            code
        })
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, runner, fin_rx, force_delay).await;
    restore_terminal();

    // If the user quit before everything exited, stop the rest, then await it.
    runner.shutdown();
    let code = run_handle.await.unwrap_or(1);
    result.map(|()| code)
}

/// The render/select loop: draw, then wait for a key, a redraw ping, or the
/// runner finishing.
async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    runner: &Runner,
    finished_rx: oneshot::Receiver<i32>,
    force_delay: Duration,
) -> io::Result<()> {
    let (key_tx, mut key_rx) = mpsc::channel::<KeyEvent>(256);
    let redraw = app.system.redraw();
    // The reader also wakes the loop on resize, so it can repaint to the new size.
    spawn_key_reader(key_tx, Arc::clone(&redraw));
    let mut finished = Some(finished_rx);
    let mut last_drawn: Option<(u16, u16, u64)> = None;
    // While `Some`, a redraw burst coalesces until this instant; keys stay responsive.
    let mut coalesce_until: Option<tokio::time::Instant> = None;

    loop {
        let size = terminal.size()?;
        // Viewport geometry from the render layer's layout (one source of truth),
        // so clamping and paging can't drift from what is drawn.
        let (inner_width, inner_height) =
            ui::viewport_size(Rect::new(0, 0, size.width, size.height));
        app.set_viewport(inner_height);
        // Clamp the scroll now the width is known (the render layer measures wrapping).
        let offset = ui::clamp_scroll(app, inner_width, inner_height);
        app.set_scroll_offset(offset);
        app.mark_seen();
        // Skip the repaint (and its merge/clone work) when nothing visible changed.
        let fingerprint = (size.width, size.height, app.render_fingerprint());
        if last_drawn != Some(fingerprint) {
            terminal.draw(|frame| ui::render(frame, app))?;
            last_drawn = Some(fingerprint);
        }
        // A received signal terminates proccie even while sitting open for review.
        if runner.quit_requested() {
            app.quit = true;
        }
        if app.quit {
            return Ok(());
        }

        tokio::select! {
            // The coalesce window elapsed: loop to repaint the accumulated changes.
            // `pending` when idle so the arm never fires (the unpolled future is safe).
            () = async {
                match coalesce_until {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                coalesce_until = None;
            }
            // First push of a burst: open a coalesce window instead of spinning.
            () = redraw.notified(), if coalesce_until.is_none() => {
                coalesce_until = Some(tokio::time::Instant::now() + REDRAW_COALESCE);
            }
            maybe_key = key_rx.recv() => {
                match maybe_key {
                    Some(key) => {
                        if app.handle_key(key, runner) {
                            arm_force_exit(force_delay);
                        }
                    }
                    None => return Ok(()),
                }
            }
            // `pending` once the run is done so the arm stays quiet without firing again.
            code = async {
                match finished.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                // Stay open after the run finishes for log review; quitting is explicit.
                app.on_runner_finished(code.ok());
                finished = None;
            }
        }
    }
}

/// Reads key presses on a dedicated OS thread (blocking `event::read`) and
/// forwards them to the async loop; a resize wakes the loop to repaint.
fn spawn_key_reader(tx: mpsc::Sender<KeyEvent>, redraw: Arc<Notify>) {
    std::thread::spawn(move || {
        loop {
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if tx.blocking_send(key).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(_, _)) => redraw.notify_one(),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
}

/// Backstops a forced shutdown (Ctrl+C raises no SIGINT in raw mode): restores
/// the terminal and hard-exits after the grace, so the process always escapes.
fn arm_force_exit(delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        restore_terminal();
        std::process::exit(1);
    });
}
