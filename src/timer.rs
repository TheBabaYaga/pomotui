//! Pure pomodoro state machine. It performs no input and no output.
//!
//! Every method that needs the time takes `now: Instant`. That keeps this
//! module pure, and it lets a test drive a virtual clock.

use std::time::{Duration, Instant};

/// The two phases. Focus and break alternate, and nothing else follows them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Focus,
    Break,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Focus => "FOCUS",
            Phase::Break => "BREAK",
        }
    }

    pub fn is_break(self) -> bool {
        matches!(self, Phase::Break)
    }

    /// The phase that follows this one.
    pub fn next(self) -> Self {
        match self {
            Phase::Focus => Phase::Break,
            Phase::Break => Phase::Focus,
        }
    }
}

/// How long the timer holds at 00:00, blinking, before the next phase starts.
/// The hold is a fixed announcement, so it runs on the wall clock.
pub const HOLD: Duration = Duration::from_secs(5);

/// The two phase lengths. `cli.rs` builds this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub focus: Duration,
    pub break_len: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            focus: Duration::from_secs(25 * 60),
            break_len: Duration::from_secs(5 * 60),
        }
    }
}

impl Config {
    pub fn duration_of(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Focus => self.focus,
            Phase::Break => self.break_len,
        }
    }
}

pub struct Timer {
    config: Config,
    phase: Phase,
    accumulated: Duration,
    running_since: Option<Instant>,
    /// Some(start) while the clock sits at 00:00 before the phase changes.
    holding_since: Option<Instant>,
}

impl Timer {
    /// A new timer waits in `Focus`. It does NOT run: the app never begins
    /// counting on its own, so the first `toggle_pause` starts it. That is also
    /// why this takes no clock reading, because nothing is timed yet.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            phase: Phase::Focus,
            accumulated: Duration::ZERO,
            running_since: None,
            holding_since: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// `Some(elapsed)` while the clock holds at 00:00 before the phase changes.
    ///
    /// This is the one source of truth for the alert. The caller blinks for as
    /// long as the hold lasts, so no second constant can drift out of step.
    pub fn held_for(&self, now: Instant) -> Option<Duration> {
        self.holding_since
            .map(|since| now.saturating_duration_since(since))
    }

    /// The clock rule: read the time, never subtract a tick.
    pub fn elapsed(&self, now: Instant) -> Duration {
        match self.running_since {
            Some(since) => self
                .accumulated
                .saturating_add(now.saturating_duration_since(since)),
            None => self.accumulated,
        }
    }

    /// Saturates at zero, so a long stall never wraps.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.config
            .duration_of(self.phase)
            .saturating_sub(self.elapsed(now))
    }

    /// 0.0 to 1.0. A zero length phase reads as complete, so the result is
    /// never NaN.
    pub fn progress(&self, now: Instant) -> f64 {
        let total = self.config.duration_of(self.phase).as_secs_f64();
        if total <= 0.0 {
            return 1.0;
        }
        (self.elapsed(now).as_secs_f64() / total).clamp(0.0, 1.0)
    }

    /// A pause banks the time that ran. A resume starts a new run at `now`.
    pub fn toggle_pause(&mut self, now: Instant) {
        match self.running_since.take() {
            Some(since) => {
                self.accumulated = self
                    .accumulated
                    .saturating_add(now.saturating_duration_since(since));
            }
            None => self.running_since = Some(now),
        }
    }

    /// The phase stays. The pause state stays too, because the user paused on
    /// purpose. A reset during the hold cancels it and starts the phase again.
    pub fn reset_phase(&mut self, now: Instant) {
        self.accumulated = Duration::ZERO;
        self.holding_since = None;
        if self.running_since.is_some() {
            self.running_since = Some(now);
        }
    }

    /// Debug only: run the clock to the end of this phase, so the next tick
    /// starts the hold. It drives the real path and not a special case.
    pub fn run_to_end(&mut self, now: Instant) {
        self.holding_since = None;
        self.accumulated = self.config.duration_of(self.phase);
        self.running_since = Some(now);
    }

    /// Move to the next phase now. It reports the phase that starts.
    pub fn skip(&mut self, now: Instant) -> Phase {
        self.advance(now)
    }

    /// Drive the clock. It reports the phase that is coming, once, at the moment
    /// this phase reaches 00:00. The change itself lands `HOLD` later, so the
    /// clock rests on 00:00 while the caller blinks.
    ///
    /// One call advances one phase at most and it drops the overflow, so a long
    /// stall lands at the start of the next phase.
    pub fn tick(&mut self, now: Instant) -> Option<Phase> {
        // The hold is a fixed announcement and not part of the countdown, so it
        // runs on the wall clock and a pause cannot freeze it.
        if let Some(since) = self.holding_since {
            if now.saturating_duration_since(since) >= HOLD {
                self.advance(now);
            }
            return None;
        }
        if !self.is_running() {
            return None;
        }
        if self.remaining(now) > Duration::ZERO {
            return None;
        }
        // Rest on 00:00, and report what is coming so the caller can sound it.
        self.holding_since = Some(now);
        Some(self.phase.next())
    }

    /// One phase change. The new phase always starts at `now`, and it runs.
    /// A pause pressed during the hold is overwritten here, because the hold is
    /// brief and the next phase always begins running.
    fn advance(&mut self, now: Instant) -> Phase {
        self.phase = self.phase.next();
        self.accumulated = Duration::ZERO;
        self.running_since = Some(now);
        self.holding_since = None;
        self.phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Short, readable durations. The rules do not depend on the real minutes.
    fn cfg() -> Config {
        Config {
            focus: Duration::from_secs(100),
            break_len: Duration::from_secs(20),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A timer already started at `now`, with `config`. A new timer no longer
    /// runs on its own, and most rules here are about a running timer.
    fn started_with(config: Config, now: Instant) -> Timer {
        let mut timer = Timer::new(config);
        timer.toggle_pause(now);
        timer
    }

    fn started(now: Instant) -> Timer {
        started_with(cfg(), now)
    }

    /// Run the current phase to its end, then through the hold, so the timer
    /// lands at the start of the next phase. It gives back what tick announced.
    fn finish_phase(timer: &mut Timer, now: &mut Instant) -> Phase {
        *now += cfg().duration_of(timer.phase());
        let coming = timer.tick(*now).expect("the phase reached zero");
        *now += HOLD;
        assert_eq!(timer.tick(*now), None, "the change itself is quiet");
        coming
    }

    #[test]
    fn a_started_timer_runs_in_focus_with_the_full_duration() {
        let base = Instant::now();
        let timer = started(base);
        assert_eq!(timer.phase(), Phase::Focus);
        assert!(timer.is_running());
        assert_eq!(timer.elapsed(base), Duration::ZERO);
        assert_eq!(timer.remaining(base), secs(100));
        assert_eq!(timer.held_for(base), None, "a new timer is not holding");
    }

    #[test]
    fn a_new_timer_waits_and_never_counts_down_on_its_own() {
        let base = Instant::now();
        let mut timer = Timer::new(cfg());

        assert_eq!(timer.phase(), Phase::Focus);
        assert!(
            !timer.is_running(),
            "the app must not start counting itself"
        );

        // The clock stays full, however long the app sits there untouched.
        assert_eq!(timer.remaining(base), secs(100));
        assert_eq!(timer.remaining(base + secs(600)), secs(100));
        assert_eq!(timer.elapsed(base + secs(600)), Duration::ZERO);
        assert_eq!(timer.progress(base + secs(600)), 0.0);

        // It never ends a phase while it waits, so nothing sounds or flashes.
        assert_eq!(timer.tick(base + secs(600)), None);
        assert_eq!(timer.held_for(base + secs(600)), None);
        assert_eq!(timer.phase(), Phase::Focus);

        // The first space starts it, counting from that moment and not from
        // whenever the app happened to boot.
        timer.toggle_pause(base + secs(600));
        assert!(timer.is_running());
        assert_eq!(timer.remaining(base + secs(600)), secs(100));
        assert_eq!(timer.remaining(base + secs(650)), secs(50));
    }

    #[test]
    fn the_remaining_time_falls_as_the_clock_advances() {
        let base = Instant::now();
        let timer = started(base);
        assert_eq!(timer.remaining(base + secs(1)), secs(99));
        assert_eq!(timer.elapsed(base + secs(1)), secs(1));
        assert_eq!(timer.remaining(base + secs(40)), secs(60));
        assert_eq!(timer.elapsed(base + secs(40)), secs(40));
        assert_eq!(timer.remaining(base + secs(99)), secs(1));
    }

    #[test]
    fn a_pause_freezes_the_remaining_time_and_a_resume_continues_from_it() {
        let base = Instant::now();
        let mut timer = started(base);

        timer.toggle_pause(base + secs(10));
        assert!(!timer.is_running());
        assert_eq!(timer.remaining(base + secs(10)), secs(90));
        // 60 seconds of pause change nothing.
        assert_eq!(timer.remaining(base + secs(70)), secs(90));
        assert_eq!(timer.elapsed(base + secs(70)), secs(10));

        timer.toggle_pause(base + secs(70));
        assert!(timer.is_running());
        assert_eq!(timer.remaining(base + secs(70)), secs(90));
        assert_eq!(timer.remaining(base + secs(75)), secs(85));
    }

    #[test]
    fn reset_phase_restores_the_full_duration_and_keeps_the_phase() {
        let base = Instant::now();
        let mut timer = started(base);

        let mut now = base + secs(30);
        assert_eq!(timer.remaining(now), secs(70));
        timer.reset_phase(now);
        assert_eq!(timer.phase(), Phase::Focus);
        assert_eq!(timer.remaining(now), secs(100));
        assert_eq!(timer.remaining(now + secs(10)), secs(90));

        // A reset keeps the pause state, because the user paused on purpose.
        now += secs(10);
        timer.toggle_pause(now);
        timer.reset_phase(now + secs(10));
        assert!(!timer.is_running());
        assert_eq!(timer.remaining(now + secs(70)), secs(100));

        // A reset inside a break keeps the break.
        let mut in_break = started(base);
        in_break.skip(base);
        assert_eq!(in_break.phase(), Phase::Break);
        in_break.reset_phase(base + secs(5));
        assert_eq!(in_break.phase(), Phase::Break);
        assert_eq!(in_break.remaining(base + secs(5)), secs(20));
    }

    #[test]
    fn skip_advances_to_the_other_phase_and_always_runs() {
        let base = Instant::now();
        let mut timer = started(base);

        assert_eq!(timer.skip(base + secs(10)), Phase::Break);
        assert_eq!(timer.phase(), Phase::Break);
        assert!(timer.is_running());
        assert_eq!(timer.remaining(base + secs(10)), secs(20));
        assert_eq!(timer.held_for(base + secs(10)), None, "a skip never holds");

        // A skipped break returns to focus, the same as a finished break.
        assert_eq!(timer.skip(base + secs(15)), Phase::Focus);
        assert_eq!(timer.remaining(base + secs(15)), secs(100));

        // A new phase always runs, even after a skip from a pause.
        timer.toggle_pause(base + secs(20));
        assert!(!timer.is_running());
        assert_eq!(timer.skip(base + secs(25)), Phase::Break);
        assert!(timer.is_running());
        assert_eq!(timer.remaining(base + secs(25)), secs(20));
        assert_eq!(timer.remaining(base + secs(30)), secs(15));
    }

    #[test]
    fn focus_and_break_alternate_and_nothing_else_ever_follows() {
        let base = Instant::now();
        let mut timer = started(base);
        let mut now = base;

        // Ten phase changes in a row. A long break must never appear.
        let expected = [
            Phase::Break,
            Phase::Focus,
            Phase::Break,
            Phase::Focus,
            Phase::Break,
            Phase::Focus,
            Phase::Break,
            Phase::Focus,
            Phase::Break,
            Phase::Focus,
        ];
        for (round, want) in expected.iter().enumerate() {
            let announced = finish_phase(&mut timer, &mut now);
            assert_eq!(announced, *want, "phase change {round} announced");
            assert_eq!(timer.phase(), *want, "phase change {round} landed");
            assert_eq!(
                timer.remaining(now),
                cfg().duration_of(*want),
                "phase change {round} starts full"
            );
        }
    }

    #[test]
    fn the_clock_holds_at_zero_for_five_seconds_before_the_phase_changes() {
        assert_eq!(HOLD, secs(5));

        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);

        assert_eq!(
            timer.tick(zero),
            Some(Phase::Break),
            "it announces the break"
        );

        // Through the whole hold the clock reads 00:00 and the phase stays put.
        for offset in [0, 1, 2_500, 4_999] {
            let at = zero + Duration::from_millis(offset);
            assert_eq!(timer.tick(at), None, "{offset} ms into the hold");
            assert_eq!(timer.phase(), Phase::Focus, "{offset} ms into the hold");
            assert_eq!(timer.remaining(at), Duration::ZERO, "{offset} ms in");
            assert_eq!(
                timer.held_for(at),
                Some(Duration::from_millis(offset)),
                "{offset} ms in"
            );
        }

        // At five seconds the break starts, full.
        let after = zero + HOLD;
        assert_eq!(timer.tick(after), None, "the change itself is quiet");
        assert_eq!(timer.phase(), Phase::Break);
        assert_eq!(timer.held_for(after), None, "the hold is over");
        assert_eq!(timer.remaining(after), secs(20));
    }

    #[test]
    fn tick_announces_the_coming_phase_once_and_only_at_zero() {
        let base = Instant::now();
        let mut timer = started(base);

        assert_eq!(timer.tick(base), None);
        assert_eq!(timer.tick(base + secs(99)), None);
        assert_eq!(timer.held_for(base + secs(99)), None);

        // One announcement at zero, and never a repeat.
        assert_eq!(timer.tick(base + secs(100)), Some(Phase::Break));
        assert_eq!(timer.tick(base + secs(100)), None);
        assert_eq!(timer.tick(base + secs(101)), None);

        // The break then announces the focus phase that follows it.
        let mut now = base + secs(100) + HOLD;
        assert_eq!(timer.tick(now), None);
        assert_eq!(timer.phase(), Phase::Break);
        now += secs(20);
        assert_eq!(timer.tick(now), Some(Phase::Focus));
    }

    #[test]
    fn a_pause_cannot_stop_the_hold() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);
        assert_eq!(timer.tick(zero), Some(Phase::Break));

        // The hold is an announcement, not a countdown, so a pause cannot
        // freeze it.
        timer.toggle_pause(zero + secs(1));
        assert!(!timer.is_running());
        assert_eq!(timer.tick(zero + secs(2)), None);
        assert_eq!(timer.phase(), Phase::Focus);
        assert_eq!(timer.held_for(zero + secs(2)), Some(secs(2)));

        // It still completes, and the next phase begins running.
        assert_eq!(timer.tick(zero + HOLD), None);
        assert_eq!(timer.phase(), Phase::Break);
        assert!(timer.is_running(), "a new phase always runs");
    }

    #[test]
    fn reset_phase_cancels_the_hold() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);
        assert_eq!(timer.tick(zero), Some(Phase::Break));
        assert!(timer.held_for(zero).is_some());

        timer.reset_phase(zero + secs(1));
        assert_eq!(
            timer.held_for(zero + secs(1)),
            None,
            "the hold is cancelled"
        );
        assert_eq!(timer.phase(), Phase::Focus, "the phase starts again");
        assert_eq!(timer.remaining(zero + secs(1)), secs(100));

        // A stale hold must not fire later.
        assert_eq!(timer.tick(zero + HOLD + secs(1)), None);
        assert_eq!(timer.phase(), Phase::Focus);
    }

    #[test]
    fn skip_during_the_hold_moves_on_at_once() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);
        assert_eq!(timer.tick(zero), Some(Phase::Break));

        assert_eq!(timer.skip(zero + secs(1)), Phase::Break);
        assert_eq!(timer.phase(), Phase::Break);
        assert_eq!(timer.held_for(zero + secs(1)), None, "the skip clears it");
        assert_eq!(timer.remaining(zero + secs(1)), secs(20));
    }

    #[test]
    fn run_to_end_brings_the_clock_to_zero_for_the_debug_key() {
        let base = Instant::now();
        let mut timer = started(base);

        timer.run_to_end(base + secs(5));
        assert_eq!(timer.remaining(base + secs(5)), Duration::ZERO);
        assert!(timer.is_running());
        // The next tick then takes the ordinary path, hold and all.
        assert_eq!(timer.tick(base + secs(5)), Some(Phase::Break));
        assert_eq!(timer.held_for(base + secs(5)), Some(Duration::ZERO));

        // It works from a pause too, so the key never looks dead.
        let mut paused = started(base);
        paused.toggle_pause(base + secs(1));
        assert!(!paused.is_running());
        paused.run_to_end(base + secs(2));
        assert!(paused.is_running());
        assert_eq!(paused.tick(base + secs(2)), Some(Phase::Break));
    }

    #[test]
    fn a_stall_during_the_hold_still_lands_at_the_start_of_the_next_phase() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);
        assert_eq!(timer.tick(zero), Some(Phase::Break));

        // The machine sleeps for an hour in the middle of the hold. The break
        // must still start full, because the hold's advance drops the overflow
        // exactly as an ordinary phase change does.
        let late = zero + secs(3600);
        assert_eq!(timer.tick(late), None);
        assert_eq!(timer.phase(), Phase::Break);
        assert_eq!(timer.held_for(late), None);
        assert_eq!(timer.elapsed(late), Duration::ZERO);
        assert_eq!(timer.remaining(late), secs(20));
        assert!(timer.is_running());
    }

    #[test]
    fn run_to_end_during_a_hold_starts_the_ending_again() {
        let base = Instant::now();
        let mut timer = started(base);
        let zero = base + secs(100);
        assert_eq!(timer.tick(zero), Some(Phase::Break));
        assert!(timer.held_for(zero).is_some());

        // The debug key during a hold clears it and ends the phase again, so
        // the key stays useful instead of doing nothing for five seconds.
        timer.run_to_end(zero + secs(1));
        assert_eq!(timer.held_for(zero + secs(1)), None, "the old hold is gone");
        assert_eq!(
            timer.phase(),
            Phase::Focus,
            "still the phase that was ending"
        );
        assert_eq!(timer.remaining(zero + secs(1)), Duration::ZERO);

        // A fresh ending follows, and the hold restarts from this moment.
        assert_eq!(timer.tick(zero + secs(1)), Some(Phase::Break));
        assert_eq!(timer.held_for(zero + secs(1)), Some(Duration::ZERO));
        assert_eq!(timer.phase(), Phase::Focus);
        // The stale hold must not cut the new one short.
        assert_eq!(timer.tick(zero + secs(1) + HOLD - secs(1)), None);
        assert_eq!(timer.phase(), Phase::Focus);
        assert_eq!(timer.tick(zero + secs(1) + HOLD), None);
        assert_eq!(timer.phase(), Phase::Break);
    }

    #[test]
    fn a_two_hour_gap_in_one_tick_lands_at_the_start_of_the_next_phase() {
        let base = Instant::now();
        let mut timer = started(base);
        let now = base + secs(7200);

        // The gap ends the phase, and the clock rests on 00:00.
        assert_eq!(timer.tick(now), Some(Phase::Break));
        assert_eq!(timer.phase(), Phase::Focus, "the change waits for the hold");
        assert_eq!(timer.remaining(now), Duration::ZERO);
        assert_eq!(timer.tick(now), None, "one call announces one ending");
        assert_eq!(timer.phase(), Phase::Focus);

        // After the hold the break starts full, and the overflow is gone.
        let after = now + HOLD;
        assert_eq!(timer.tick(after), None);
        assert_eq!(timer.phase(), Phase::Break);
        assert_eq!(timer.elapsed(after), Duration::ZERO);
        assert_eq!(timer.remaining(after), secs(20));
    }

    #[test]
    fn remaining_saturates_at_zero_and_never_wraps() {
        let base = Instant::now();
        let mut timer = started(base);
        // A pause holds the elapsed time far past the end of the phase.
        timer.toggle_pause(base + secs(500));
        assert_eq!(timer.elapsed(base + secs(500)), secs(500));
        assert_eq!(timer.remaining(base + secs(500)), Duration::ZERO);
        assert_eq!(timer.remaining(base + secs(100_000)), Duration::ZERO);
        assert_eq!(timer.elapsed(base + secs(100_000)), secs(500));

        let running = started(base);
        assert_eq!(running.remaining(base + secs(100)), Duration::ZERO);
        assert_eq!(running.remaining(base + secs(100_000)), Duration::ZERO);
    }

    #[test]
    fn a_paused_timer_does_not_advance_across_a_phase_boundary() {
        let base = Instant::now();
        let mut timer = started(base);
        timer.toggle_pause(base + secs(30));

        assert_eq!(timer.tick(base + secs(9999)), None);
        assert_eq!(timer.phase(), Phase::Focus);
        assert_eq!(timer.remaining(base + secs(9999)), secs(70));
        assert_eq!(timer.held_for(base + secs(9999)), None, "no hold starts");

        // After a resume the phase still owes its last 70 seconds.
        let resumed = base + secs(9999);
        timer.toggle_pause(resumed);
        assert_eq!(timer.tick(resumed + secs(69)), None);
        assert_eq!(timer.tick(resumed + secs(70)), Some(Phase::Break));

        // A pause that banks more than the phase duration also blocks the
        // announcement, even though nothing remains.
        let mut late = started(base);
        late.toggle_pause(base + secs(500));
        assert_eq!(late.remaining(base + secs(500)), Duration::ZERO);
        assert_eq!(late.tick(base + secs(500)), None);
        assert_eq!(late.tick(base + secs(600)), None);
        assert_eq!(late.phase(), Phase::Focus);

        // Only the resume lets the phase end.
        late.toggle_pause(base + secs(600));
        assert_eq!(late.tick(base + secs(600)), Some(Phase::Break));
    }

    #[test]
    fn progress_stays_between_zero_and_one() {
        let base = Instant::now();
        let mut timer = started(base);
        assert_eq!(timer.progress(base), 0.0);
        assert!((timer.progress(base + secs(25)) - 0.25).abs() < 1e-9);
        assert!((timer.progress(base + secs(50)) - 0.5).abs() < 1e-9);
        assert_eq!(timer.progress(base + secs(100)), 1.0);
        // Far past the end it stops at 1.0.
        assert_eq!(timer.progress(base + secs(100_000)), 1.0);

        timer.toggle_pause(base + secs(4000));
        let paused = timer.progress(base + secs(4000));
        assert!((0.0..=1.0).contains(&paused), "progress was {paused}");
        assert_eq!(paused, 1.0);

        // A zero length phase must not give NaN.
        let zero = started_with(
            Config {
                focus: Duration::ZERO,
                ..cfg()
            },
            base,
        );
        let value = zero.progress(base);
        assert!((0.0..=1.0).contains(&value), "progress was {value}");
    }

    #[test]
    fn the_labels_and_the_break_flag_match_the_phase() {
        assert_eq!(Phase::Focus.label(), "FOCUS");
        assert_eq!(Phase::Break.label(), "BREAK");
        assert!(!Phase::Focus.is_break());
        assert!(Phase::Break.is_break());
        assert_eq!(Phase::Focus.next(), Phase::Break);
        assert_eq!(Phase::Break.next(), Phase::Focus);
    }

    #[test]
    fn the_default_config_is_twenty_five_and_five_minutes() {
        let config = Config::default();
        assert_eq!(config.focus, secs(25 * 60));
        assert_eq!(config.break_len, secs(5 * 60));
        assert_eq!(config.duration_of(Phase::Focus), secs(25 * 60));
        assert_eq!(config.duration_of(Phase::Break), secs(5 * 60));
    }
}
