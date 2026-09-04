//! What a worktree and its branch are called.
//!
//! A session used to be named by its ULID, which is unique and says nothing:
//! a rail full of `01k4grpc…` is a rail nobody can tell apart. A name here
//! is two words — a quality and a thing — drawn from two lists of fifty, so
//! there are exactly 2,500 of them and every one can be read aloud. The
//! lists lean on what the company is named for: umbra, penumbra, eclipse,
//! dusk, and the iconoclast who breaks what is put in front of them.
//!
//! Uniqueness is per repository and checked against git, not assumed: the
//! name is chosen from those no branch of the repository already carries,
//! starting from a random position so two repositories do not all begin at
//! `ashen-umbra`. Only when every name is taken does the ULID come back, as a
//! suffix, so a repository with 2,501 sessions still gets a branch.

use std::collections::HashSet;

/// Fifty qualities. Adjectival, one word, lowercase, no hyphen.
pub const ADJECTIVES: [&str; 50] = [
    "umbral",
    "penumbral",
    "dusky",
    "eclipsed",
    "shadowed",
    "nocturnal",
    "veiled",
    "hushed",
    "tenebrous",
    "crepuscular",
    "moonlit",
    "starless",
    "ashen",
    "sable",
    "obsidian",
    "smouldering",
    "sunless",
    "lightless",
    "twilit",
    "waning",
    "waxing",
    "occluded",
    "latent",
    "silent",
    "unseen",
    "hidden",
    "hollow",
    "feral",
    "unbowed",
    "unbound",
    "defiant",
    "heretical",
    "errant",
    "rogue",
    "restless",
    "stubborn",
    "contrary",
    "iron",
    "wild",
    "unruly",
    "wayward",
    "dissenting",
    "unquiet",
    "sovereign",
    "untamed",
    "brazen",
    "sceptical",
    "unrepentant",
    "irreverent",
    "oblique",
];

/// Fifty things. Nominal, one word, lowercase, no hyphen, none of them also
/// an adjective above — a test holds both lists to that.
pub const NOUNS: [&str; 50] = [
    "umbra",
    "penumbra",
    "shadow",
    "eclipse",
    "dusk",
    "nightfall",
    "gloaming",
    "totality",
    "corona",
    "occultation",
    "syzygy",
    "aphelion",
    "perihelion",
    "zenith",
    "nadir",
    "ember",
    "cinder",
    "ash",
    "veil",
    "shroud",
    "lantern",
    "candle",
    "moth",
    "raven",
    "owl",
    "wolf",
    "fox",
    "comet",
    "meteor",
    "void",
    "abyss",
    "grotto",
    "cavern",
    "iconoclast",
    "heretic",
    "rebel",
    "dissenter",
    "apostate",
    "maverick",
    "renegade",
    "insurgent",
    "outlier",
    "vanguard",
    "hammer",
    "chisel",
    "schism",
    "rupture",
    "verdict",
    "manifesto",
    "tide",
];

/// How many names there are: every quality with every thing.
pub const COUNT: usize = ADJECTIVES.len() * NOUNS.len();

/// The `i`th name, for `i` below [`COUNT`]. Stable: the same index is the
/// same name in every build, which is what lets a random start be a random
/// start rather than a random list.
pub fn name(i: usize) -> String {
    let i = i % COUNT;
    format!("{}-{}", ADJECTIVES[i / NOUNS.len()], NOUNS[i % NOUNS.len()])
}

/// The first name, walking from `start`, that `taken` does not hold — or
/// `None` when every one of the 2,500 is taken.
pub fn pick(taken: &HashSet<String>, start: usize) -> Option<String> {
    (0..COUNT)
        .map(|k| name((start + k) % COUNT))
        .find(|candidate| !taken.contains(candidate))
}

/// A readable name no branch of `repo` carries under `prefix` and no
/// directory under `dir` holds, from a random start so consecutive sessions
/// are not consecutive names. When all 2,500 are taken, the first free name
/// is made by suffixing a ULID, which cannot collide.
pub async fn fresh(repo: &std::path::Path, dir: &std::path::Path, prefix: &str) -> String {
    let mut taken: HashSet<String> = crate::git::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}"),
        ],
    )
    .await
    .map(|out| {
        out.lines()
            .filter_map(|line| line.trim().strip_prefix(prefix))
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(dir) {
        taken.extend(
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok()),
        );
    }
    let ulid = ulid::Ulid::generate();
    let start = (ulid.random() % COUNT as u128) as usize;
    pick(&taken, start)
        .unwrap_or_else(|| format!("{}-{}", name(start), ulid.to_string().to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_exactly_2500_names_and_no_two_are_the_same() {
        assert_eq!(COUNT, 2500);
        let all: HashSet<String> = (0..COUNT).map(name).collect();
        assert_eq!(all.len(), COUNT);
    }

    #[test]
    fn every_word_is_one_lowercase_word_and_no_word_is_on_both_lists() {
        let adjectives: HashSet<&str> = ADJECTIVES.iter().copied().collect();
        let nouns: HashSet<&str> = NOUNS.iter().copied().collect();
        assert_eq!(adjectives.len(), ADJECTIVES.len(), "a quality repeats");
        assert_eq!(nouns.len(), NOUNS.len(), "a thing repeats");
        assert!(adjectives.is_disjoint(&nouns), "a word is on both lists");
        for word in adjectives.iter().chain(nouns.iter()) {
            assert!(
                !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()),
                "`{word}` is not one lowercase word"
            );
        }
    }

    #[test]
    fn a_name_is_a_valid_branch_and_directory_name() {
        for i in [0, 1, 49, 50, 2499] {
            let n = name(i);
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{n}");
            assert!(
                !n.starts_with('-') && !n.ends_with('-') && !n.contains("--"),
                "{n}"
            );
        }
    }

    #[test]
    fn pick_skips_what_is_taken_and_wraps_around() {
        let mut taken = HashSet::new();
        assert_eq!(pick(&taken, 7), Some(name(7)));
        taken.insert(name(7));
        assert_eq!(pick(&taken, 7), Some(name(8)));
        // Starting at the last name, the walk wraps to the first.
        assert_eq!(pick(&taken, COUNT - 1), Some(name(COUNT - 1)));
        taken.insert(name(COUNT - 1));
        assert_eq!(pick(&taken, COUNT - 1), Some(name(0)));
        // Every name taken: nothing to pick.
        let all: HashSet<String> = (0..COUNT).map(name).collect();
        assert_eq!(pick(&all, 3), None);
    }
}
