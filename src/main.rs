//! Terminal setup, the event loop, the alert, and the flash timing.
//!
//! Every other module is pure. All input and output stays here.

mod cli;
mod digits;
mod timer;
mod ui;

use std::io::Write;
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::timer::{Config, Phase, Timer};

/// The redraw rate. `poll` returns at once when a key arrives, so this timeout
/// never slows a key down.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// How long one blink lasts. Two redraws cover one blink, so the flash reads as
/// motion. Motion catches the eye better than a colour that just sits there.
///
/// The flash LENGTH is not set here. It lasts exactly as long as the timer holds
/// at 00:00, so there is no second constant that can drift out of step.
const BLINK: Duration = Duration::from_millis(500);

/// The terminal bell, as one byte. Many terminals turn this into a silent
/// visual mark, so it is the fallback and never the only alert.
const BELL: &[u8] = b"\x07";

/// The sound player. It ships with macOS. A system without it stays quiet.
const PLAYER: &str = "afplay";
/// A focus phase ended, so a break starts.
const SOUND_BREAK: &str = "/System/Library/Sounds/Glass.aiff";
/// A break ended, so a focus phase starts.
const SOUND_FOCUS: &str = "/System/Library/Sounds/Ping.aiff";

/// What one key press asks the app to do.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    TogglePause,
    Reset,
    Skip,
    Quit,
    /// Debug only: run the clock to 00:00, so the ordinary path takes over.
    Alert,
}

fn main() -> ExitCode {
    // Parse first, so clap prints a usage error to a plain terminal and sets
    // the exit code before the terminal changes mode.
    let args = cli::Args::parse();
    let config = args.to_config();

    // `try_init` installs the same restoring panic hook as `init`, so a panic
    // is still covered. Unlike `init` it hands the setup failure back, so a run
    // with no terminal ends in one clear line instead of a panic.
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            // Raw mode can already be on when a later setup step failed.
            let _ = ratatui::try_restore();
            eprintln!("pomotui: this app needs a terminal ({error})");
            return ExitCode::FAILURE;
        }
    };

    let result = run(terminal, config, args.debug);
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pomotui: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The event loop. One pass draws the screen, reads at most one event, and
/// ticks the timer once.
fn run(mut terminal: DefaultTerminal, config: Config, debug: bool) -> std::io::Result<()> {
    let mut timer = Timer::new(config);
    let mut player: Option<Child> = None;

    loop {
        // One reading of the clock for the whole pass, so the tick and the
        // frame that follows it agree.
        let now = Instant::now();

        // Collect a finished player, so a long session leaves no zombie.
        reap(&mut player);

        // Tick BEFORE drawing. Drawing first shows a frame of stale state, and
        // the blink is what suffers: the opening blink comes out half length,
        // and one lit frame outlives the hold.
        //
        // The phase reached 00:00. It rests there and blinks for the hold, so
        // sound the phase that is coming.
        if let Some(starting) = timer.tick(now) {
            alert(starting, &mut player);
        }

        let view = build_view(&timer, config, debug, now);
        terminal.draw(|frame| ui::draw(frame, &view))?;

        // `poll` returns at once when a key arrives, and the next pass redraws
        // straight away, so a key still feels immediate.
        if event::poll(POLL_TIMEOUT)?
            && let Some(action) = action_for(&event::read()?, debug)
        {
            match action {
                Action::TogglePause => timer.toggle_pause(now),
                Action::Reset => timer.reset_phase(now),
                // A skip moves on at once and stays quiet. You pressed the
                // key, so you already know the phase changed.
                Action::Skip => {
                    timer.skip(now);
                }
                // Debug only: run the clock out. The next tick then holds,
                // sounds, and blinks exactly as a real phase end does.
                Action::Alert => timer.run_to_end(now),
                Action::Quit => {
                    // Leave no sound playing after the app is gone.
                    stop(&mut player);
                    return Ok(());
                }
            }
        }
    }
}

/// The frame snapshot. This is the only place that maps the `Timer` onto the
/// `View`.
fn build_view(timer: &Timer, config: Config, debug: bool, now: Instant) -> ui::View {
    let phase = timer.phase();
    ui::View {
        phase_label: phase.label(),
        is_break: phase.is_break(),
        remaining: timer.remaining(now),
        progress: timer.progress(now),
        paused: !timer.is_running(),
        flashing: is_flashing(timer.held_for(now)),
        debug: if debug {
            Some(debug_line(timer, config, now))
        } else {
            None
        },
    }
}

/// The live state line, so a debug run shows what the state machine believes.
fn debug_line(timer: &Timer, config: Config, now: Instant) -> String {
    let phase = timer.phase();
    let state = match timer.held_for(now) {
        Some(held) => format!("holding {:.1}s", held.as_secs_f64()),
        None if timer.is_running() => "running".to_string(),
        None => "paused".to_string(),
    };
    format!(
        "{}  {:.1}s / {:.1}s  {state}",
        phase.label().to_lowercase(),
        timer.elapsed(now).as_secs_f64(),
        config.duration_of(phase).as_secs_f64(),
    )
}

/// True while the flash is lit. The timer reports how long it has held at
/// 00:00, and the blink alternates every `BLINK` from the start of that hold.
///
/// Counting from the START matters. Counting down from a deadline starts the
/// alert DARK, because integer division puts the first drawn frame in the odd
/// bucket and no frame ever sees the full remaining time.
fn is_flashing(held_for: Option<Duration>) -> bool {
    let Some(done) = held_for else {
        return false;
    };
    (done.as_millis() / BLINK.as_millis()).is_multiple_of(2)
}

/// The key map. Only a key press earns an action: a terminal that reports a
/// key release would otherwise act twice for one keypress.
fn action_for(event: &Event, debug: bool) -> Option<Action> {
    let Event::Key(key) = event else {
        return None;
    };
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        // Raw mode swallows Ctrl+C, so this arm is the only way out for a user
        // who reaches for it. Without it the app traps the user.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        KeyCode::Char(' ') => Some(Action::TogglePause),
        KeyCode::Char('r') => Some(Action::Reset),
        KeyCode::Char('s') => Some(Action::Skip),
        KeyCode::Char('a') if debug => Some(Action::Alert),
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        _ => None,
    }
}

/// A phase ended: sound the phase that is coming. The blink is not started
/// here. It follows the timer's own hold, so the two can never disagree.
fn alert(starting: Phase, player: &mut Option<Child>) {
    ring_bell();
    play(sound_for(starting), player);
}

/// Two sounds, so you know which phase started without looking at the screen.
fn sound_for(starting: Phase) -> &'static str {
    match starting {
        Phase::Break => SOUND_BREAK,
        Phase::Focus => SOUND_FOCUS,
    }
}

/// One bell byte, flushed. Without the flush the byte can sit in the buffer
/// and never sound.
fn ring_bell() {
    // A lost beep must never kill the app, so both errors drop here.
    let mut out = std::io::stdout();
    let _ = out.write_all(BELL);
    let _ = out.flush();
}

/// Stop the player and collect it, so no process is ever left behind. A player
/// that already finished is only collected.
fn stop(player: &mut Option<Child>) {
    if let Some(mut child) = player.take() {
        match child.try_wait() {
            // It finished, and `try_wait` collected it.
            Ok(Some(_)) => {}
            // It still runs, or the check failed. Stop it and collect it.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Start the sound. A player that is still busy gives way to the new alert.
fn play(sound: &str, player: &mut Option<Child>) {
    stop(player);

    // The player is missing on any system that is not macOS. The app then
    // stays quiet, which beats a crash, and the bell and the flash remain.
    *player = Command::new(PLAYER)
        .arg(sound)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
}

/// Collect a player that has finished.
fn reap(player: &mut Option<Child>) {
    if let Some(child) = player.as_mut()
        && matches!(child.try_wait(), Ok(Some(_)))
    {
        *player = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::crossterm::event::KeyEvent;

    use crate::timer::HOLD;

    /// Short, readable durations. The key map does not depend on the minutes.
    fn cfg() -> Config {
        Config {
            focus: Duration::from_secs(100),
            break_len: Duration::from_secs(20),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A timer already started at `now`. A new timer waits for the first space.
    fn started(now: Instant) -> Timer {
        let mut timer = Timer::new(cfg());
        timer.toggle_pause(now);
        timer
    }

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press_with(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn of_kind(code: KeyCode, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind))
    }

    #[test]
    fn space_r_and_s_map_to_pause_reset_and_skip() {
        for debug in [false, true] {
            assert_eq!(
                action_for(&press(KeyCode::Char(' ')), debug),
                Some(Action::TogglePause)
            );
            assert_eq!(
                action_for(&press(KeyCode::Char('r')), debug),
                Some(Action::Reset)
            );
            assert_eq!(
                action_for(&press(KeyCode::Char('s')), debug),
                Some(Action::Skip)
            );
        }
    }

    #[test]
    fn q_esc_and_ctrl_c_all_quit() {
        for debug in [false, true] {
            assert_eq!(
                action_for(&press(KeyCode::Char('q')), debug),
                Some(Action::Quit)
            );
            assert_eq!(action_for(&press(KeyCode::Esc), debug), Some(Action::Quit));
            // Raw mode swallows Ctrl+C, so the key map must catch it.
            assert_eq!(
                action_for(
                    &press_with(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    debug
                ),
                Some(Action::Quit)
            );
        }
    }

    #[test]
    fn the_alert_key_works_only_in_debug_mode() {
        assert_eq!(
            action_for(&press(KeyCode::Char('a')), true),
            Some(Action::Alert)
        );
        assert_eq!(action_for(&press(KeyCode::Char('a')), false), None);
    }

    #[test]
    fn a_plain_c_does_nothing_and_an_unknown_key_does_nothing() {
        for debug in [false, true] {
            assert_eq!(action_for(&press(KeyCode::Char('c')), debug), None);
            assert_eq!(action_for(&press(KeyCode::Char('x')), debug), None);
            assert_eq!(action_for(&press(KeyCode::Enter), debug), None);
            assert_eq!(action_for(&press(KeyCode::Up), debug), None);
        }
    }

    #[test]
    fn only_a_key_press_earns_an_action() {
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            for code in [
                KeyCode::Char(' '),
                KeyCode::Char('r'),
                KeyCode::Char('s'),
                KeyCode::Char('a'),
                KeyCode::Char('q'),
                KeyCode::Esc,
            ] {
                assert_eq!(
                    action_for(&of_kind(code, kind), true),
                    None,
                    "{code:?} of kind {kind:?} must do nothing"
                );
            }
        }

        // A Ctrl+C release must not quit twice either.
        assert_eq!(
            action_for(
                &Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL,
                    KeyEventKind::Release,
                )),
                false
            ),
            None
        );
    }

    #[test]
    fn an_event_that_is_not_a_key_does_nothing() {
        assert_eq!(action_for(&Event::Resize(80, 24), false), None);
        assert_eq!(action_for(&Event::FocusGained, false), None);
        assert_eq!(action_for(&Event::FocusLost, false), None);
    }

    #[test]
    fn the_flash_blinks_from_the_start_of_the_hold() {
        assert!(!is_flashing(None), "no hold, no flash");

        // Lit for the first blink, so the alert is seen at once.
        for ms in [0, 1, 100, 249, 250, 499] {
            assert!(
                is_flashing(Some(Duration::from_millis(ms))),
                "must be lit {ms} ms into the hold"
            );
        }
        for ms in [500, 501, 750, 999] {
            assert!(
                !is_flashing(Some(Duration::from_millis(ms))),
                "must be dark {ms} ms into the hold"
            );
        }
        for ms in [1_000, 1_250, 1_499] {
            assert!(
                is_flashing(Some(Duration::from_millis(ms))),
                "must be lit again {ms} ms into the hold"
            );
        }

        // Ten whole blinks fill the five second hold, and it starts lit.
        let pattern: Vec<bool> = (0..10).map(|n| is_flashing(Some(BLINK * n))).collect();
        assert_eq!(
            pattern,
            vec![
                true, false, true, false, true, false, true, false, true, false
            ]
        );
    }

    #[test]
    fn the_flash_covers_exactly_the_hold_and_then_stops() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);

        // Nothing flashes while the phase still runs.
        assert!(!build_view(&timer, cfg(), false, base + secs(50)).flashing);

        // The phase reaches 00:00. The first frame after it must be lit.
        assert_eq!(timer.tick(zero), Some(Phase::Break));
        assert!(
            build_view(&timer, cfg(), false, zero).flashing,
            "the alert has to be visible at once"
        );
        assert!(build_view(&timer, cfg(), false, zero + POLL_TIMEOUT).flashing);

        // The clock rests on 00:00, still on the phase that ended.
        let held = build_view(&timer, cfg(), false, zero + secs(2));
        assert_eq!(held.remaining, Duration::ZERO);
        assert_eq!(held.phase_label, "FOCUS", "the change waits for the hold");
        assert!((held.progress - 1.0).abs() < 1e-9, "the gauge reads full");

        // When the hold ends, the flash ends with it and the break starts.
        assert_eq!(timer.tick(zero + HOLD), None);
        let after = build_view(&timer, cfg(), false, zero + HOLD);
        assert!(!after.flashing, "the flash stops with the hold");
        assert_eq!(after.phase_label, "BREAK");
        assert_eq!(after.remaining, secs(20));
    }

    #[test]
    fn each_starting_phase_has_its_own_sound() {
        assert_eq!(sound_for(Phase::Break), SOUND_BREAK);
        assert_eq!(sound_for(Phase::Focus), SOUND_FOCUS);
        assert_ne!(
            sound_for(Phase::Break),
            sound_for(Phase::Focus),
            "the two phases must sound different"
        );
    }

    #[test]
    fn the_app_boots_stopped_with_a_full_clock() {
        let base = Instant::now();
        let timer = Timer::new(cfg());

        // However long the app sits at the prompt, nothing has counted down.
        let view = build_view(&timer, cfg(), false, base + secs(600));
        assert!(view.paused, "the app must boot stopped");
        assert_eq!(view.remaining, secs(100), "the clock has not moved");
        assert_eq!(view.progress, 0.0);
        assert!(!view.flashing);
        assert_eq!(view.phase_label, "FOCUS");
    }

    #[test]
    fn the_view_mirrors_the_timer() {
        let base = Instant::now();
        let timer = started(base);
        let view = build_view(&timer, cfg(), false, base + secs(25));

        assert_eq!(view.phase_label, "FOCUS");
        assert!(!view.is_break);
        assert_eq!(view.remaining, secs(75));
        assert!(
            (view.progress - 0.25).abs() < 1e-9,
            "progress was {}",
            view.progress
        );
        assert!(!view.paused);
        assert!(!view.flashing);
        assert_eq!(view.debug, None, "no debug line without the flag");
    }

    #[test]
    fn a_paused_timer_reads_as_paused_in_the_view() {
        let base = Instant::now();
        let mut timer = started(base);

        // The second press pauses it, because the first one started it.
        timer.toggle_pause(base);
        let paused = build_view(&timer, cfg(), false, base + secs(60));
        assert!(paused.paused);
        // A pause freezes the clock the view shows.
        assert_eq!(paused.remaining, secs(100));

        timer.toggle_pause(base + secs(60));
        assert!(!build_view(&timer, cfg(), false, base + secs(60)).paused);
    }

    #[test]
    fn a_break_phase_reaches_the_view_as_a_break() {
        let base = Instant::now();
        let mut timer = started(base);
        timer.skip(base);

        let view = build_view(&timer, cfg(), false, base);
        assert_eq!(view.phase_label, "BREAK");
        assert!(view.is_break);
        assert_eq!(view.remaining, secs(20));
        assert!(!view.flashing, "a skip never flashes");
    }

    #[test]
    fn the_debug_line_reports_the_phase_the_clock_and_the_state() {
        let base = Instant::now();
        let mut timer = started(base);

        let running = build_view(&timer, cfg(), true, base + secs(25))
            .debug
            .expect("debug mode must fill the line");
        assert!(running.contains("focus"), "{running}");
        assert!(running.contains("25.0s / 100.0s"), "{running}");
        assert!(running.contains("running"), "{running}");

        timer.toggle_pause(base + secs(25));
        let paused = build_view(&timer, cfg(), true, base + secs(90))
            .debug
            .expect("debug mode must fill the line");
        assert!(paused.contains("paused"), "{paused}");
        // The pause froze the elapsed time at 25 seconds.
        assert!(paused.contains("25.0s / 100.0s"), "{paused}");

        // The hold shows itself, so a debug run can watch the five seconds go.
        timer.run_to_end(base + secs(90));
        assert_eq!(timer.tick(base + secs(90)), Some(Phase::Break));
        let holding = build_view(&timer, cfg(), true, base + secs(92))
            .debug
            .expect("debug mode must fill the line");
        assert!(holding.contains("holding 2.0s"), "{holding}");
        assert!(holding.contains("focus"), "{holding}");
    }

    #[test]
    fn the_loop_timings_fit_together() {
        // Only the relationships between the constants are worth a test. Their
        // values are asserted where they mean something: HOLD in timer.rs, and
        // the blink pattern in the two flash tests. Comparing a constant to its
        // own literal here would only pad the count.

        // Two redraws must cover one blink, or the flash aliases.
        assert!(
            BLINK >= POLL_TIMEOUT * 2,
            "blink {BLINK:?} against poll {POLL_TIMEOUT:?}"
        );
        // The hold must be a whole number of blinks, so it never ends mid-blink.
        assert!(
            HOLD.as_millis().is_multiple_of(BLINK.as_millis()),
            "hold {HOLD:?} must be whole blinks of {BLINK:?}"
        );
    }
}
