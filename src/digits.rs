//! Block glyphs for the large clock. Pure, and it has no dependency.

/// Rows that one rendered clock occupies.
pub const HEIGHT: usize = 5;

/// Columns that one digit glyph occupies.
const DIGIT_WIDTH: usize = 3;
/// Columns that the colon glyph occupies.
const COLON_WIDTH: usize = 1;
/// Blank columns between two glyphs.
const GAP: usize = 1;

/// One pixel that is on. This is U+2588 FULL BLOCK.
const ON: char = '\u{2588}';
/// One pixel that is off.
const OFF: char = ' ';

/// The mark for an on pixel in the pattern table.
const LIT: char = '#';
/// The mark for an off pixel in the pattern table. Only the table check reads
/// it, because `push_pattern_row` treats every mark that is not `LIT` as off.
#[cfg(test)]
const DARK: char = '.';

/// One pattern row for each of the `HEIGHT` rows of one glyph.
type Pattern = [&'static str; HEIGHT];

/// The glyph table, from 0 to 9. Every row is `DIGIT_WIDTH` marks wide.
const DIGIT_PATTERNS: [Pattern; 10] = [
    ["###", "#.#", "#.#", "#.#", "###"],
    ["..#", "..#", "..#", "..#", "..#"],
    ["###", "..#", "###", "#..", "###"],
    ["###", "..#", "###", "..#", "###"],
    ["#.#", "#.#", "###", "..#", "..#"],
    ["###", "#..", "###", "..#", "###"],
    ["###", "#..", "###", "#.#", "###"],
    ["###", "..#", "..#", "..#", "..#"],
    ["###", "#.#", "###", "#.#", "###"],
    ["###", "#.#", "###", "..#", "###"],
];

/// The colon glyph. Every row is `COLON_WIDTH` marks wide.
const COLON_PATTERN: Pattern = [".", "#", ".", "#", "."];

/// Render `total_secs` as `MM:SS` in block glyphs.
///
/// It returns exactly `HEIGHT` lines, and every line has the same length.
/// It renders three minute digits once the minutes reach 100, because
/// `--focus 120` is legal.
pub fn render_time(total_secs: u64) -> Vec<String> {
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;

    let mut glyphs: Vec<(&Pattern, usize)> = Vec::new();
    for digit in minute_digits(minutes) {
        glyphs.push((&DIGIT_PATTERNS[digit], DIGIT_WIDTH));
    }
    glyphs.push((&COLON_PATTERN, COLON_WIDTH));
    // The remainder of 10 keeps both indexes inside the table.
    glyphs.push((&DIGIT_PATTERNS[(seconds / 10) as usize], DIGIT_WIDTH));
    glyphs.push((&DIGIT_PATTERNS[(seconds % 10) as usize], DIGIT_WIDTH));

    (0..HEIGHT)
        .map(|row| {
            let mut line = String::new();
            for (index, (pattern, width)) in glyphs.iter().enumerate() {
                if index > 0 {
                    push_repeat(&mut line, OFF, GAP);
                }
                push_pattern_row(&mut line, pattern[row], *width);
            }
            line
        })
        .collect()
}

/// The minute digits, most significant first. It gives two digits below 100
/// minutes, and one more digit for each power of ten above that.
fn minute_digits(minutes: u64) -> Vec<usize> {
    let mut digits = Vec::new();
    let mut left = minutes;
    while left > 0 {
        // The remainder of 10 keeps the index inside the table.
        digits.push((left % 10) as usize);
        left /= 10;
    }
    while digits.len() < 2 {
        digits.push(0);
    }
    digits.reverse();
    digits
}

/// Write exactly `width` pixels. That keeps every line the same length, even
/// if a pattern row is the wrong size.
fn push_pattern_row(line: &mut String, pattern_row: &str, width: usize) {
    let mut written = 0;
    for mark in pattern_row.chars().take(width) {
        line.push(if mark == LIT { ON } else { OFF });
        written += 1;
    }
    push_repeat(line, OFF, width.saturating_sub(written));
}

fn push_repeat(line: &mut String, pixel: char, count: usize) {
    for _ in 0..count {
        line.push(pixel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four values the spec names, plus the boundary that adds a digit.
    const SAMPLES: [u64; 6] = [0, 59, 1500, 7200, 5999, 6000];

    #[test]
    fn render_time_returns_height_lines_of_equal_length() {
        for total_secs in SAMPLES {
            let lines = render_time(total_secs);

            assert_eq!(
                lines.len(),
                HEIGHT,
                "{total_secs} seconds must give {HEIGHT} lines"
            );

            let width = lines[0].chars().count();
            assert!(width > 0, "{total_secs} seconds must give a real width");
            for (row, line) in lines.iter().enumerate() {
                assert_eq!(
                    line.chars().count(),
                    width,
                    "row {row} of {total_secs} seconds must match row 0"
                );
            }
        }
    }

    #[test]
    fn render_time_draws_the_expected_block_glyphs() {
        assert_eq!(
            render_time(0),
            vec![
                "███ ███   ███ ███",
                "█ █ █ █ █ █ █ █ █",
                "█ █ █ █   █ █ █ █",
                "█ █ █ █ █ █ █ █ █",
                "███ ███   ███ ███",
            ]
        );
    }

    #[test]
    fn render_time_uses_three_minute_digits_from_one_hundred_minutes() {
        let two_digits = render_time(5999); // 99:59
        let three_digits = render_time(6000); // 100:00

        assert_eq!(two_digits[0].chars().count(), 17);
        assert_eq!(
            three_digits[0].chars().count(),
            17 + DIGIT_WIDTH + GAP,
            "the third minute digit adds one glyph and one gap"
        );

        // 120 minutes is legal, because `--focus 120` is legal.
        assert_eq!(
            render_time(120 * 60)[0].chars().count(),
            three_digits[0].chars().count()
        );
    }

    #[test]
    fn render_time_uses_only_the_block_and_the_space() {
        for total_secs in SAMPLES {
            for line in render_time(total_secs) {
                for pixel in line.chars() {
                    assert!(
                        pixel == ON || pixel == OFF,
                        "{pixel:?} is not a pixel of {total_secs} seconds"
                    );
                }
            }
        }
    }

    #[test]
    fn the_glyph_table_holds_only_well_formed_rows() {
        let mut patterns: Vec<(&str, &[&str; HEIGHT], usize)> = Vec::new();
        for pattern in &DIGIT_PATTERNS {
            patterns.push(("digit", pattern, DIGIT_WIDTH));
        }
        patterns.push(("colon", &COLON_PATTERN, COLON_WIDTH));

        for (name, pattern, width) in patterns {
            for row in pattern {
                assert_eq!(
                    row.chars().count(),
                    width,
                    "{name} row {row:?} is the wrong width"
                );
                for mark in row.chars() {
                    assert!(
                        mark == LIT || mark == DARK,
                        "{name} row {row:?} holds {mark:?}"
                    );
                }
            }
        }
    }
}
