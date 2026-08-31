//! One pure draw function. It reads a `View` snapshot, and nothing else.
//! It never touches the `Timer`, and it holds no state.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge};

use crate::digits;

/// Focus red, pinned to an explicit value.
///
/// `Color::Red` is ANSI index 1, and the terminal THEME decides what that is.
/// Catppuccin Mocha, the theme in use on this machine, paints index 1 as
/// #fe428a: blue at 138 against green at 66, which reads as hot pink. An
/// indexed colour is a request the theme may reinterpret, so a colour that
/// carries meaning has to name its own value.
const FOCUS: Color = Color::Rgb(224, 49, 49);

/// Break green. This one stays theme-defined on purpose, because the theme
/// green already reads as green.
const BREAK: Color = Color::Green;

/// The border title.
const TITLE: &str = " pomotui ";
/// The mark that follows the phase label while the timer is paused.
const PAUSED: &str = "  PAUSED";
/// The key reminder on the bottom row, while the clock runs.
const HELP: &str = "space pause  r reset  s skip  q quit";
/// The same row while the clock is stopped. The app boots stopped, so the row
/// has to say how to start before it says how to pause.
const HELP_PAUSED: &str = "space start  r reset  s skip  q quit";
/// The key reminder in debug mode, which adds the alert key.
const HELP_DEBUG: &str = "space pause  r reset  s skip  a alert  q quit";
const HELP_PAUSED_DEBUG: &str = "space start  r reset  s skip  a alert  q quit";

/// FROZEN. `main.rs` builds this each frame. Do not change it.
#[derive(Clone, Debug)]
pub struct View {
    pub phase_label: &'static str,
    pub is_break: bool,
    pub remaining: Duration,
    pub progress: f64,
    pub paused: bool,
    /// True while the phase-change flash is lit. `main.rs` blinks it.
    pub flashing: bool,
    /// The live state line, in debug mode only.
    pub debug: Option<String>,
}

/// How many rows each part of the screen gets.
///
/// The clock keeps one row for the plain `MM:SS`. Each other line then takes
/// one row while a row is spare, in the order label, gauge, help, debug. The
/// clock grows into every row that is left. So the plan spends the whole
/// height, and it drops the lines from the end of that order first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowPlan {
    label: bool,
    clock_rows: u16,
    gauge: bool,
    help: bool,
    debug: bool,
}

impl RowPlan {
    fn new(rows: u16, wants_debug: bool) -> Self {
        if rows == 0 {
            return Self {
                label: false,
                clock_rows: 0,
                gauge: false,
                help: false,
                debug: false,
            };
        }

        // One row belongs to the clock. The rest is spare.
        let mut spare = rows.saturating_sub(1);
        let label = take_row(&mut spare);
        let gauge = take_row(&mut spare);
        let help = take_row(&mut spare);
        // The short circuit leaves the row spare when debug is off, so the
        // clock keeps it.
        let debug = wants_debug && take_row(&mut spare);

        Self {
            label,
            clock_rows: spare.saturating_add(1),
            gauge,
            help,
            debug,
        }
    }
}

/// Spend one spare row. It reports false when no row is spare.
fn take_row(spare: &mut u16) -> bool {
    if *spare == 0 {
        return false;
    }
    *spare = spare.saturating_sub(1);
    true
}

pub fn draw(frame: &mut Frame, view: &View) {
    let area = frame.area();
    draw_content(frame, area, view);

    // The lit flash paints the background LAST, over every widget already
    // drawn. A background change is far easier to catch than one reversed word,
    // and it leaves the glyphs undistorted.
    //
    // Last, and not first, because a widget may set its own background and so
    // punch a hole in the fill. `Gauge` does exactly that on the cells under
    // its label, and it covers one cell MORE than the label it then draws, so
    // no label style can close that gap. This style carries only a background,
    // and `Cell::set_style` patches only the fields that are set, so every
    // symbol and every foreground survives.
    if view.flashing {
        frame.render_widget(
            Block::new().style(Style::new().bg(phase_colour(view))),
            area,
        );
    }
}

/// Everything except the flash fill. It may return early on an area too small
/// to hold anything, which is why the fill lives in `draw` and not here.
fn draw_content(frame: &mut Frame, area: Rect, view: &View) {
    let block = Block::bordered()
        .title(TITLE)
        .border_style(border_style(view));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let plan = RowPlan::new(inner.height, view.debug.is_some());
    let mut top = inner.y;

    if plan.label {
        frame.render_widget(label_line(view), band(inner, top, 1));
        top = top.saturating_add(1);
    }

    draw_clock(frame, band(inner, top, plan.clock_rows), view);
    top = top.saturating_add(plan.clock_rows);

    if plan.gauge {
        frame.render_widget(progress_gauge(view), band(inner, top, 1));
        top = top.saturating_add(1);
    }

    if plan.debug {
        if let Some(text) = view.debug.as_deref()
            && fits_width(text, inner.width)
        {
            frame.render_widget(
                Line::from(Span::styled(text, quiet_style(view, Color::DarkGray))).centered(),
                band(inner, top, 1),
            );
        }
        top = top.saturating_add(1);
    }

    if plan.help {
        let help = help_text(view);
        if fits_width(help, inner.width) {
            frame.render_widget(
                Line::from(Span::styled(help, quiet_style(view, Color::Reset))).centered(),
                band(inner, top, 1),
            );
        }
    }
}

fn help_text(view: &View) -> &'static str {
    match (view.paused, view.debug.is_some()) {
        (false, false) => HELP,
        (true, false) => HELP_PAUSED,
        (false, true) => HELP_DEBUG,
        (true, true) => HELP_PAUSED_DEBUG,
    }
}

/// True when the whole text fits. A line that only half fits drops out, so it
/// never scribbles a part of itself across a narrow screen.
fn fits_width(text: &str, width: u16) -> bool {
    text.chars().count() <= usize::from(width)
}

/// `height` full width rows of `within`, from row `y`. The result never leaves
/// `within`, so a widget can never draw past the bottom.
fn band(within: Rect, y: u16, height: u16) -> Rect {
    let used = y.saturating_sub(within.y);
    let left = within.height.saturating_sub(used);
    Rect {
        x: within.x,
        y,
        width: within.width,
        height: height.min(left),
    }
}

/// The middle row of `within`.
fn middle_row(within: Rect) -> Rect {
    let y = within.y.saturating_add(within.height.saturating_sub(1) / 2);
    band(within, y, 1)
}

/// The border takes the text colour, so it stays readable on the lit flash.
fn border_style(view: &View) -> Style {
    if view.flashing {
        Style::new()
            .fg(text_colour(view))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    }
}

fn label_line(view: &View) -> Line<'static> {
    let style = Style::new()
        .fg(text_colour(view))
        .add_modifier(Modifier::BOLD);

    let mut spans = vec![Span::styled(view.phase_label, style)];
    if view.paused {
        spans.push(Span::styled(PAUSED, style));
    }
    Line::from(spans).centered()
}

fn clock_style(view: &View) -> Style {
    Style::new()
        .fg(text_colour(view))
        .add_modifier(Modifier::BOLD)
}

/// `Gauge` needs care to keep the lit flash whole.
///
/// It sets a background on every cell it fills, defaulting to `Color::Reset`, so
/// the flash colour has to be handed to it. Worse, it SWAPS foreground and
/// background on the filled cells that sit under its own label, which leaves
/// those cells black. During a hold the ratio is always 1.0, because the phase
/// just completed, so the label always sits over the filled bar and that hole
/// always shows. An explicit label with an explicit background closes it.
fn progress_gauge(view: &View) -> Gauge<'static> {
    let ratio = safe_ratio(view.progress);
    let gauge = Gauge::default().ratio(ratio);

    if !view.flashing {
        // Calm: let the gauge invert its own label, which is the usual look.
        return gauge.gauge_style(Style::new().fg(phase_colour(view)));
    }

    let style = Style::new().fg(text_colour(view)).bg(phase_colour(view));
    gauge
        .gauge_style(style)
        .label(Span::styled(format!("{}%", (ratio * 100.0).round()), style))
}

/// The colour that carries the phase. It is the text colour normally, and the
/// background colour while the flash is lit.
fn phase_colour(view: &View) -> Color {
    if view.is_break { BREAK } else { FOCUS }
}

/// The text colour. The lit flash fills the background with the phase colour,
/// so the text turns black to stay readable.
fn text_colour(view: &View) -> Color {
    if view.flashing {
        Color::Black
    } else {
        phase_colour(view)
    }
}

/// A quiet line. It turns black while the flash fills the background.
fn quiet_style(view: &View, calm: Color) -> Style {
    if view.flashing {
        Style::new().fg(Color::Black)
    } else {
        Style::new().fg(calm)
    }
}

/// `Gauge::ratio` panics outside 0.0 to 1.0, and a plain clamp keeps a NaN. So
/// every value that is not a real ratio reads as zero here.
fn safe_ratio(progress: f64) -> f64 {
    if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The large clock when the real glyph block fits. Otherwise a plain `MM:SS`.
fn draw_clock(frame: &mut Frame, area: Rect, view: &View) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let style = clock_style(view);
    let total_secs = display_secs(view.remaining);
    let glyphs = digits::render_time(total_secs);
    // Measure the rendered lines, never a guessed width.
    let glyph_width = glyphs
        .iter()
        .map(|line| line.chars().count())
        .fold(0, usize::max);

    let fits = glyphs.len() == digits::HEIGHT
        && glyph_width > 0
        && glyph_width <= usize::from(area.width)
        && digits::HEIGHT <= usize::from(area.height);

    if !fits {
        frame.render_widget(
            Line::from(Span::styled(plain_time(total_secs), style)).centered(),
            middle_row(area),
        );
        return;
    }

    // Take one row of the area for each glyph row. Half of what is left is the
    // top margin, so the block sits in the middle of the area.
    let mut spare = area.height;
    for _ in &glyphs {
        spare = spare.saturating_sub(1);
    }

    let mut y = area.y.saturating_add(spare / 2);
    for line in &glyphs {
        frame.render_widget(
            Line::from(Span::styled(line.as_str(), style)).centered(),
            band(area, y, 1),
        );
        y = y.saturating_add(1);
    }
}

fn plain_time(total_secs: u64) -> String {
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// Round the remaining time up. A 25 minute phase then reads 25:00 for its
/// first whole second, and 00:00 never sits on screen while the phase runs.
/// Truncation would show 24:59 microseconds after the start.
fn display_secs(remaining: Duration) -> u64 {
    remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::digits;

    fn focus_view() -> View {
        View {
            phase_label: "FOCUS",
            is_break: false,
            remaining: Duration::from_secs(1500),
            progress: 0.5,
            paused: false,
            flashing: false,
            debug: None,
        }
    }

    fn render(width: u16, height: u16, view: &View) -> Terminal<TestBackend> {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend never fails");
        terminal
            .draw(|frame| draw(frame, view))
            .expect("the test backend never fails");
        terminal
    }

    fn screen(width: u16, height: u16, view: &View) -> String {
        render(width, height, view).backend().to_string()
    }

    /// The first cell of the phase label on the top row of the inner area.
    fn label_cell(terminal: &Terminal<TestBackend>, first: &str) -> ratatui::buffer::Cell {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width;
        (0..width)
            .map(|x| buffer[(x, 1)].clone())
            .find(|cell| cell.symbol() == first)
            .expect("the label must reach the top inner row")
    }

    /// The top left border cell.
    fn border_cell(terminal: &Terminal<TestBackend>) -> ratatui::buffer::Cell {
        terminal.backend().buffer()[(0, 0)].clone()
    }

    /// Cells whose background is `colour`.
    fn background_count(terminal: &Terminal<TestBackend>, colour: Color) -> usize {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.bg == colour)
            .count()
    }

    /// Cells drawn with reverse video. The flash must never use it: reversing a
    /// block glyph swaps its ink for its paper and the digits look broken.
    fn reversed_cells(terminal: &Terminal<TestBackend>) -> usize {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .count()
    }

    #[test]
    fn a_44x16_render_shows_the_phase_label_and_the_gauge() {
        let out = screen(44, 16, &focus_view());

        assert!(
            out.contains("pomotui"),
            "the border title is missing:\n{out}"
        );
        assert!(out.contains("FOCUS"), "the phase label is missing:\n{out}");
        assert!(out.contains('%'), "the gauge percentage is missing:\n{out}");
        assert!(out.contains(HELP), "the help line is missing:\n{out}");
    }

    #[test]
    fn the_cycle_counter_is_gone_for_good() {
        for height in 1..=24u16 {
            let out = screen(60, height, &focus_view());
            assert!(
                !out.contains("pomodoro"),
                "the counter must not draw at 60x{height}:\n{out}"
            );
            assert!(
                !out.contains(" of "),
                "no 'N of M' text may remain at 60x{height}:\n{out}"
            );
        }
    }

    #[test]
    fn a_44x16_render_shows_the_large_clock_digits() {
        let out = screen(44, 16, &focus_view());

        for line in digits::render_time(1500) {
            assert!(
                out.contains(line.trim_end()),
                "the glyph row {line:?} is missing:\n{out}"
            );
        }
    }

    #[test]
    fn a_30x8_render_falls_back_to_a_plain_mm_ss() {
        let out = screen(30, 8, &focus_view());

        assert!(out.contains("25:00"), "the plain clock is missing:\n{out}");
        let top_glyph_row = digits::render_time(1500)[0].clone();
        assert!(
            !out.contains(top_glyph_row.trim_end()),
            "the large clock must not draw at 30x8:\n{out}"
        );
    }

    #[test]
    fn a_12x4_render_does_not_panic() {
        let out = screen(12, 4, &focus_view());

        assert_eq!(out.lines().count(), 4);
        assert!(out.contains("25:00"), "the plain clock is missing:\n{out}");
    }

    #[test]
    fn every_size_from_1x1_to_60x24_draws_without_a_panic() {
        let mut view = focus_view();
        for width in 1..=60u16 {
            for height in 1..=24u16 {
                // Flip the flags with the size, so every style path draws.
                view.paused = width % 2 == 0;
                view.flashing = height % 2 == 0;
                view.is_break = width % 3 == 0;
                view.debug = if width % 5 == 0 {
                    Some(format!("focus 12s left  running  {width}x{height}"))
                } else {
                    None
                };
                view.remaining = Duration::from_secs(u64::from(width) * 137);
                let out = screen(width, height, &view);
                assert_eq!(
                    out.lines().count(),
                    usize::from(height),
                    "{width}x{height} drew the wrong number of rows"
                );
            }
        }
    }

    #[test]
    fn a_paused_view_shows_paused_next_to_the_label() {
        let mut view = focus_view();
        view.paused = true;
        let out = screen(44, 16, &view);

        assert!(out.contains("FOCUS  PAUSED"), "no pause mark:\n{out}");

        let running = screen(44, 16, &focus_view());
        assert!(!running.contains("PAUSED"), "a running view is not paused");
    }

    #[test]
    fn the_lit_flash_fills_the_whole_background() {
        let cells = 44 * 16;

        // Ratio 1.0 FIRST, because it is the only ratio a real flash ever runs
        // at: the hold begins the moment the phase completes. That is also
        // where the gauge inverts its label over the filled bar, so a hole in
        // the fill shows there and nowhere else.
        for progress in [1.0, 0.75, 0.5, 0.0] {
            let mut view = focus_view();
            view.progress = progress;
            let calm = render(44, 16, &view);
            view.flashing = true;
            let lit = render(44, 16, &view);

            assert_eq!(
                background_count(&lit, FOCUS),
                cells,
                "at ratio {progress} every cell must take the flash background"
            );
            // The calm gauge legitimately inverts its label, so a few cells may
            // carry the colour. The window must simply not be filled.
            assert!(
                background_count(&calm, FOCUS) < cells / 4,
                "at ratio {progress} the calm window must not be filled"
            );
            assert_eq!(reversed_cells(&lit), 0, "at ratio {progress}");
        }

        let mut view = focus_view();
        view.progress = 1.0;
        let calm = render(44, 16, &view);
        view.flashing = true;
        let lit = render(44, 16, &view);

        // The text turns black so it stays readable on the fill.
        assert_eq!(label_cell(&lit, "F").fg, Color::Black, "label");
        assert!(label_cell(&lit, "F").modifier.contains(Modifier::BOLD));
        assert_eq!(border_cell(&lit).fg, Color::Black, "border");
        assert_eq!(label_cell(&calm, "F").fg, FOCUS, "calm label");

        // Nothing is reversed any more. That is what distorted the glyphs.
        assert_eq!(reversed_cells(&lit), 0, "the flash must not reverse a cell");
        assert_eq!(reversed_cells(&calm), 0);

        // A break flashes green, so the colour names the phase that started.
        view.is_break = true;
        view.phase_label = "BREAK";
        assert_eq!(background_count(&render(44, 16, &view), BREAK), cells);
    }

    #[test]
    fn the_clock_rounds_the_remaining_time_up() {
        assert_eq!(display_secs(Duration::ZERO), 0);
        assert_eq!(display_secs(Duration::from_secs(1500)), 1500);
        // Any fraction rounds up, so a phase never reads one second short.
        assert_eq!(display_secs(Duration::from_nanos(1)), 1);
        assert_eq!(display_secs(Duration::from_millis(1_499_999)), 1500);
        assert_eq!(display_secs(Duration::from_micros(2_500_001)), 3);

        // The drawn clock follows. A 25 minute phase reads 25:00 on its first
        // frame, and it reaches 00:00 only at the boundary.
        let mut view = focus_view();
        view.remaining = Duration::from_millis(1_499_999);
        assert!(screen(30, 8, &view).contains("25:00"));
        view.remaining = Duration::from_millis(2_001);
        assert!(screen(30, 8, &view).contains("00:03"));
        view.remaining = Duration::ZERO;
        assert!(screen(30, 8, &view).contains("00:00"));
    }

    #[test]
    fn the_label_is_red_for_focus_and_green_for_a_break() {
        let focus = label_cell(&render(44, 16, &focus_view()), "F");
        assert_eq!(focus.fg, FOCUS);

        let mut view = focus_view();
        view.phase_label = "BREAK";
        view.is_break = true;
        let rest = label_cell(&render(44, 16, &view), "B");
        assert_eq!(rest.fg, BREAK);
    }

    #[test]
    fn the_focus_colour_is_a_real_red_that_no_theme_can_repaint() {
        // Color::Red is ANSI index 1. Catppuccin Mocha paints it #fe428a, which
        // reads as pink, so an indexed colour cannot carry this meaning.
        assert_ne!(FOCUS, Color::Red, "an indexed colour follows the theme");

        let Color::Rgb(r, g, b) = FOCUS else {
            panic!("the focus colour must name its own value, got {FOCUS:?}");
        };

        // Red has to dominate.
        assert!(r > 150, "too dark to read as red, r={r}");
        assert!(
            u16::from(r) > u16::from(g) * 2,
            "red must dominate green, r={r} g={g}"
        );
        // Blue leading green is exactly what makes a colour look pink.
        assert!(b <= g, "blue above green reads as pink, g={g} b={b}");
        assert!(b < 120, "too much blue reads as pink, b={b}");
    }

    #[test]
    fn the_debug_line_draws_only_in_debug_mode_and_adds_its_key() {
        let mut view = focus_view();
        view.debug = Some("focus  24:58 left  running".to_string());
        let out = screen(60, 16, &view);

        assert!(
            out.contains("24:58 left"),
            "the debug line is missing:\n{out}"
        );
        assert!(
            out.contains("a alert"),
            "the debug help must name the alert key:\n{out}"
        );

        let plain = screen(60, 16, &focus_view());
        assert!(!plain.contains("running"), "no debug line without the flag");
        assert!(
            !plain.contains("a alert"),
            "the alert key must stay hidden:\n{plain}"
        );
        assert!(plain.contains(HELP));
    }

    #[test]
    fn the_help_row_says_start_while_the_clock_is_stopped() {
        let mut view = focus_view();
        view.paused = true;
        let stopped = screen(50, 16, &view);
        assert!(stopped.contains("space start"), "{stopped}");
        assert!(!stopped.contains("space pause"), "{stopped}");

        let running = screen(50, 16, &focus_view());
        assert!(running.contains("space pause"), "{running}");
        assert!(!running.contains("space start"), "{running}");

        // Debug mode keeps the alert key and the right first word together.
        view.debug = Some("focus  0.0s / 3.0s  paused".to_string());
        let both = screen(60, 16, &view);
        assert!(both.contains("space start"), "{both}");
        assert!(both.contains("a alert"), "{both}");
    }

    #[test]
    fn the_row_plan_spends_every_row_and_drops_the_lines_in_order() {
        assert_eq!(
            RowPlan::new(0, true),
            RowPlan {
                label: false,
                clock_rows: 0,
                gauge: false,
                help: false,
                debug: false,
            }
        );

        // The clock keeps the last row, then each line takes one.
        assert_eq!(RowPlan::new(1, false).clock_rows, 1);
        assert!(!RowPlan::new(1, false).label);
        assert!(RowPlan::new(2, false).label);
        assert!(!RowPlan::new(2, false).gauge);
        assert!(RowPlan::new(3, false).gauge);
        assert!(!RowPlan::new(3, false).help);
        assert!(RowPlan::new(4, false).help);

        // The debug row comes last, and only when it is wanted.
        assert!(!RowPlan::new(4, true).debug);
        assert!(RowPlan::new(5, true).debug);
        assert!(!RowPlan::new(9, false).debug);
        // A spare row the debug line does not take goes to the clock.
        assert_eq!(RowPlan::new(5, false).clock_rows, 2);
        assert_eq!(RowPlan::new(5, true).clock_rows, 1);

        for rows in 0..=200u16 {
            for wants_debug in [false, true] {
                let plan = RowPlan::new(rows, wants_debug);
                let spent = plan.clock_rows
                    + u16::from(plan.label)
                    + u16::from(plan.gauge)
                    + u16::from(plan.help)
                    + u16::from(plan.debug);
                assert_eq!(
                    spent, rows,
                    "{rows} rows, debug {wants_debug}: the plan must spend them all"
                );
            }
        }
    }

    #[test]
    fn a_wild_progress_value_never_reaches_the_gauge() {
        assert_eq!(safe_ratio(f64::NAN), 0.0);
        assert_eq!(safe_ratio(f64::INFINITY), 0.0);
        assert_eq!(safe_ratio(f64::NEG_INFINITY), 0.0);
        assert_eq!(safe_ratio(-1.0), 0.0);
        assert_eq!(safe_ratio(9.0), 1.0);
        assert_eq!(safe_ratio(0.25), 0.25);

        // A bad progress value must not panic inside the gauge.
        let mut view = focus_view();
        view.progress = f64::NAN;
        let out = screen(44, 16, &view);
        assert!(out.contains("0%"), "the gauge must read zero:\n{out}");
    }

    #[test]
    fn a_narrow_render_drops_the_lines_that_do_not_fit() {
        // The help line needs 35 columns, and 30x8 gives 28.
        let narrow = screen(30, 8, &focus_view());
        assert!(
            !narrow.contains("r reset"),
            "a help line that does not fit must drop:\n{narrow}"
        );
        assert!(
            narrow.contains("25:00"),
            "the clock always draws:\n{narrow}"
        );

        // A debug line that does not fit drops the same way.
        let mut view = focus_view();
        view.debug = Some("a very long debug line that cannot fit".to_string());
        let tight = screen(20, 10, &view);
        assert!(
            !tight.contains("very long debug"),
            "a debug line that does not fit must drop:\n{tight}"
        );

        // A wide screen keeps the help line.
        let wide = screen(44, 16, &focus_view());
        assert!(wide.contains(HELP));
    }
}
