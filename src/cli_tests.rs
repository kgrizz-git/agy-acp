//! Tests for command-line parsing.

use crate::Cli;
use clap::Parser;

#[test]
fn test_parse_skip_naration_flag() {
    assert!(
        Cli::try_parse_from(["agy-acp", "--skip-naration"])
            .unwrap()
            .skip_naration
    );
    assert!(!Cli::try_parse_from(["agy-acp"]).unwrap().skip_naration);
    assert!(Cli::try_parse_from(["agy-acp", "--skip-narration"]).is_err());
}
