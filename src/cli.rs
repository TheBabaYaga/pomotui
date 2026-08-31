//! Command line arguments, and the map to `Config`.

use std::time::Duration;

use clap::Parser;

use crate::timer::Config;

/// Seconds in one unit of `--focus` and `--break`. Debug mode counts seconds,
/// so a whole phase runs in the time it takes to watch it.
const NORMAL_UNIT: u64 = 60;
const DEBUG_UNIT: u64 = 1;

#[derive(Parser, Debug)]
#[command(
    name = "pomotui",
    version,
    about = "A small pomodoro timer for the terminal"
)]
pub struct Args {
    /// Focus phase length, in minutes
    #[arg(short, long, default_value_t = 25, value_parser = clap::value_parser!(u64).range(1..=1440))]
    pub focus: u64,

    /// Break length, in minutes
    #[arg(short = 'b', long = "break", default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=1440))]
    pub break_len: u64,

    /// Read --focus and --break as SECONDS, and add the 'a' key to fire the
    /// phase-end alert on demand. For testing the app without the wait
    #[arg(long)]
    pub debug: bool,
}

impl Args {
    pub fn to_config(&self) -> Config {
        let unit = self.unit();
        Config {
            focus: Duration::from_secs(self.focus * unit),
            break_len: Duration::from_secs(self.break_len * unit),
        }
    }

    /// Seconds in one unit of the duration flags.
    fn unit(&self) -> u64 {
        if self.debug { DEBUG_UNIT } else { NORMAL_UNIT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn minutes(m: u64) -> Duration {
        Duration::from_secs(m * 60)
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn no_argument_gives_the_spec_defaults() {
        let args = Args::try_parse_from(["pomotui"]).expect("the bare program name must parse");

        assert_eq!(args.focus, 25);
        assert_eq!(args.break_len, 5);
        assert!(!args.debug);

        let config = args.to_config();

        assert_eq!(config.focus, minutes(25));
        assert_eq!(config.break_len, minutes(5));
    }

    #[test]
    fn every_flag_parses_long_and_short_and_converts_minutes() {
        let expected = Config {
            focus: minutes(50),
            break_len: minutes(10),
        };

        let long = Args::try_parse_from(["pomotui", "--focus", "50", "--break", "10"])
            .expect("the long flags must parse");
        assert_eq!(long.to_config(), expected);

        let short = Args::try_parse_from(["pomotui", "-f", "50", "-b", "10"])
            .expect("the short flags must parse");
        assert_eq!(short.to_config(), expected);

        // One flag at a time keeps the other at its default.
        let only_focus =
            Args::try_parse_from(["pomotui", "-f", "120"]).expect("one short flag must parse");
        assert_eq!(only_focus.to_config().focus, minutes(120));
        assert_eq!(only_focus.to_config().break_len, minutes(5));

        let only_break =
            Args::try_parse_from(["pomotui", "--break", "7"]).expect("one long flag must parse");
        assert_eq!(only_break.to_config().break_len, minutes(7));
        assert_eq!(only_break.to_config().focus, minutes(25));

        // The top of the range parses.
        let top = Args::try_parse_from(["pomotui", "-f", "1440", "-b", "1440"])
            .expect("the top of the range parses");
        assert_eq!(top.to_config().focus, minutes(1440));
    }

    #[test]
    fn debug_mode_reads_the_same_flags_as_seconds() {
        let args = Args::try_parse_from(["pomotui", "--debug", "--focus", "5", "--break", "3"])
            .expect("the debug flags must parse");

        assert!(args.debug);
        assert_eq!(
            args.to_config(),
            Config {
                focus: secs(5),
                break_len: secs(3),
            }
        );

        // The same argv without --debug means minutes, so the flag is the only
        // thing that changes the unit.
        let normal = Args::try_parse_from(["pomotui", "--focus", "5", "--break", "3"])
            .expect("the plain flags must parse");
        assert_eq!(
            normal.to_config(),
            Config {
                focus: minutes(5),
                break_len: minutes(3),
            }
        );

        // Debug on its own keeps the defaults, read as seconds.
        let bare = Args::try_parse_from(["pomotui", "--debug"]).expect("--debug alone must parse");
        assert_eq!(
            bare.to_config(),
            Config {
                focus: secs(25),
                break_len: secs(5),
            }
        );
    }

    #[test]
    fn zero_and_over_range_values_are_rejected() {
        for argv in [["pomotui", "--focus", "0"], ["pomotui", "--break", "0"]] {
            assert!(
                Args::try_parse_from(argv).is_err(),
                "zero must fail for {argv:?}"
            );
        }

        for argv in [
            ["pomotui", "--focus", "1441"],
            ["pomotui", "--break", "1441"],
        ] {
            assert!(
                Args::try_parse_from(argv).is_err(),
                "a value over the range must fail for {argv:?}"
            );
        }

        // A zero stays rejected in debug mode too.
        assert!(Args::try_parse_from(["pomotui", "--debug", "--focus", "0"]).is_err());
    }

    #[test]
    fn the_long_break_and_cycle_flags_are_gone() {
        for argv in [
            ["pomotui", "--long-break", "15"],
            ["pomotui", "--cycles", "4"],
            ["pomotui", "-l", "15"],
            ["pomotui", "-c", "4"],
        ] {
            assert!(
                Args::try_parse_from(argv).is_err(),
                "{argv:?} must no longer be accepted"
            );
        }
    }

    #[test]
    fn the_clap_surface_is_well_formed() {
        Args::command().debug_assert();
    }
}
