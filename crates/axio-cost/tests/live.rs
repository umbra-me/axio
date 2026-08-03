//! Reads the session logs of whoever is running the suite.
//!
//! Ignored by default, for the same reason `axio-provider`'s live checks are: it asserts
//! against files this repository does not own and cannot fixture. A machine with no
//! agents installed would fail it for being clean, which is not a defect.
//!
//! ```sh
//! cargo test -p axio-cost --test live -- --ignored --nocapture
//! ```
//!
//! A stub written from a format description parses a fixture written from the same
//! description, so unit tests alone cannot catch a misunderstanding of the real format.
//! These assertions are the ones a wrong parser fails.

use axio_cost::pricing::Prices;
use axio_cost::sources::{registry, scan};
use axio_cost::totals::{Cost, Totals, render};

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

#[test]
#[ignore = "reads the running user's own agent logs"]
fn the_local_logs_parse_into_priced_sessions() {
    let Some(home) = home() else {
        panic!("no HOME or USERPROFILE to scan")
    };

    let report = scan(&home, &registry());
    let prices = Prices::bundled();
    let mut any_installed = false;
    let mut grand = Totals::default();

    for agent in &report.agents {
        if !agent.present {
            println!("{:<14} not installed", agent.display_name);
            continue;
        }
        any_installed = true;

        let mut totals = Totals::default();
        totals.extend(&agent.messages, &prices);

        println!(
            "{:<14} {:>5} files {:>7} billable {:>4} bad {:>14} tokens  {}",
            agent.display_name,
            agent.files_read,
            agent.outcome.billable,
            agent.outcome.malformed,
            totals.tokens.total(),
            render(&totals.cost()),
        );
        if !totals.unpriced_models.is_empty() {
            println!("               unpriced: {:?}", totals.unpriced_models);
        }

        if agent.files_read > 0 {
            assert!(
                agent.outcome.billable > 0,
                "{} read {} files and found no usage",
                agent.display_name,
                agent.files_read,
            );
        }
        let lines = agent.outcome.billable + agent.outcome.skipped + agent.outcome.malformed;
        if lines > 100 {
            let bad = 100.0 * agent.outcome.malformed as f64 / lines as f64;
            assert!(
                bad < 5.0,
                "{}: {bad:.1}% of lines unparsable",
                agent.display_name
            );
        }

        // The defect this test exists to catch: a figure derived from a negligible slice
        // of the volume, printed as though it were the agent's cost.
        if let Cost::Partial { dollars, covered } = totals.cost() {
            assert!(
                covered >= 0.01,
                "{} showed ${dollars:.2} from {:.4}% of its tokens",
                agent.display_name,
                covered * 100.0,
            );
        }

        grand.merge(&totals);
    }

    assert!(
        any_installed,
        "no agent logs found under {}",
        home.display()
    );
    println!(
        "
all agents: {} over {} messages",
        render(&grand.cost()),
        grand.messages
    );
    println!(
        "{} of {} tokens priced",
        grand.tokens.total() - grand.unpriced_tokens,
        grand.tokens.total()
    );

    for message in report.messages() {
        assert!(
            message.is_billable(),
            "an unbillable message reached the report"
        );
    }
}
