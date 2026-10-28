use std::ffi::OsStr;
use std::io::{self, Write};

use crate::cli::Cli;

pub(crate) const BANNER: &str = concat!(
    "  +------           azbridge\n",
    "  |                 Relay control plane\n",
    "             |\n",
    "       ------+      Outbound paths. Clear control.\n",
);

pub(crate) fn should_show_banner(
    cli: &Cli,
    stderr_is_terminal: bool,
    no_banner: Option<&OsStr>,
    log_level: Option<&str>,
) -> bool {
    stderr_is_terminal
        && !cli.quiet
        && !cli.tui
        && !cli.svc
        && !cli.svc_install
        && !cli.svc_uninstall
        && cli.command.is_none()
        && no_banner != Some(OsStr::new("1"))
        && !log_level.is_some_and(|level| {
            level.eq_ignore_ascii_case("QUIET") || level.eq_ignore_ascii_case("NONE")
        })
}

pub(crate) fn write_banner(output: &mut impl Write) -> io::Result<()> {
    output.write_all(BANNER.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn banner_is_ascii_width_bounded_and_matches_reviewed_design() {
        assert!(BANNER.is_ascii());
        assert!(BANNER.lines().all(|line| line.len() <= 72));
        assert!(BANNER.contains("azbridge"));
        assert!(BANNER.contains("Relay control plane"));
        assert!(BANNER.contains("Outbound paths. Clear control."));
        let design = include_str!("../tools/brand/build_final_board.py");
        assert!(BANNER.lines().all(|line| design.contains(line)));
    }

    #[test]
    fn interactive_foreground_start_shows_banner() {
        let cli = Cli::parse_from(["azbridge"]);
        assert!(should_show_banner(&cli, true, None, None));
        assert!(should_show_banner(
            &cli,
            true,
            Some(OsStr::new("0")),
            Some("DEBUG")
        ));
    }

    #[test]
    fn non_terminal_and_environment_opt_out_suppress_banner() {
        let cli = Cli::parse_from(["azbridge"]);
        assert!(!should_show_banner(&cli, false, None, None));
        assert!(!should_show_banner(&cli, true, Some(OsStr::new("1")), None));
        for level in ["QUIET", "quiet", "NONE", "none"] {
            assert!(!should_show_banner(&cli, true, None, Some(level)));
        }
    }

    #[test]
    fn quiet_tui_and_all_service_commands_suppress_banner() {
        for flag in ["-q", "--quiet", "--tui", "--svc", "-I", "-U"] {
            let cli = Cli::parse_from(["azbridge", flag]);
            assert!(!should_show_banner(&cli, true, None, None), "{flag}");
        }
    }

    #[test]
    fn managed_commands_suppress_banner() {
        for args in [
            vec!["azbridge", "status"],
            vec!["azbridge", "logout"],
            vec![
                "azbridge",
                "login",
                "--broker",
                "https://broker.example.com",
                "--workspace",
                "work",
                "--profile",
                "profile",
            ],
        ] {
            let cli = Cli::parse_from(args);
            assert!(!should_show_banner(&cli, true, None, None));
        }
    }

    #[test]
    fn writer_only_receives_the_banner_and_propagates_errors() {
        let mut stderr = Vec::new();
        write_banner(&mut stderr).unwrap();
        assert_eq!(stderr, BANNER.as_bytes());
        let error = write_banner(&mut io::Cursor::new([0_u8; 1])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }

    #[test]
    fn root_help_contains_banner_without_altering_version_or_subcommand_help() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains(BANNER));
        assert!(help.contains("Usage:"));
        let version = Cli::try_parse_from(["azbridge", "--version"])
            .unwrap_err()
            .to_string();
        assert!(!version.contains("Outbound paths."));
        let help = Cli::try_parse_from(["azbridge", "status", "--help"])
            .unwrap_err()
            .to_string();
        assert!(!help.contains("Outbound paths."));
    }
}
