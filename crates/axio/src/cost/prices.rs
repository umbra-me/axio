//! The price table `axio cost` runs against: the bundled one, plus any imported feed.
//!
//! Split out of the command itself because importing a feed is a separate job from
//! reporting on one, and the two share only a path.

use axio_cost::pricing::{Prices, feed};

use super::home_dir;

/// Where a refreshed price feed is kept once imported.
pub(super) fn overlay_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".axio").join("prices.json")
}

/// Import a price feed so models the bundled table has never heard of can be costed.
///
/// Deliberately a file rather than a fetch. `axio-provider` is the only crate here that
/// links HTTP, and one convenience is not worth spending that boundary on — so the
/// download is a job for whatever already speaks HTTP on this machine:
///
/// ```sh
/// curl -fsSL https://models.dev/api.json -o prices.json
/// axio cost --import-prices prices.json
/// ```
///
/// The document is parsed before it is stored, so an unreadable feed fails here rather
/// than silently becoming an empty overlay that prices nothing.
pub fn import_prices(path: &std::path::Path) -> u8 {
    let Some(home) = home_dir() else {
        eprintln!("axio: no home directory");
        return 1;
    };
    let document = match std::fs::read_to_string(path) {
        Ok(document) => document,
        Err(err) => {
            eprintln!("axio: cannot read {}: {err}", path.display());
            return 1;
        }
    };
    let rates = match feed::parse(&document) {
        Ok(rates) => rates,
        Err(err) => {
            eprintln!("axio: {} is not a price feed: {err}", path.display());
            return 1;
        }
    };

    let destination = overlay_path(&home);
    if let Some(parent) = destination.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!("axio: {err}");
        return 1;
    }
    if let Err(err) = std::fs::write(&destination, &document) {
        eprintln!("axio: {err}");
        return 1;
    }
    println!(
        "{} rates imported to {}",
        rates.len(),
        destination.display()
    );
    0
}

/// The bundled table, plus any imported feed.
///
/// A feed that has gone missing or unreadable since import is ignored rather than fatal:
/// the bundled table still prices most of what anyone runs, and refusing to report at all
/// because an optional file moved would be the wrong trade.
pub(super) fn prices_for(home: &std::path::Path) -> Prices {
    let bundled = Prices::bundled();
    let Ok(document) = std::fs::read_to_string(overlay_path(home)) else {
        return bundled;
    };
    match feed::parse(&document) {
        Ok(rates) => bundled.with_overlay("imported feed", rates),
        Err(_) => bundled,
    }
}
