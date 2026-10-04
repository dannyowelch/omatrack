//! Loads the freely licensed modules in `tests/data`.
//!
//! Artist, license, source, and sha256 for each file are in
//! `tests/data/ATTRIBUTION.txt`. See `tests/data/README.md`.
//!
//! Every fixture in this set is a 31-sample `M.K.` module, including
//! `openmpt_test_BSD3.mod` (OpenMPT's loader test, `test/test.mod`). The
//! writer stores that layout losslessly, so parse-then-write must match the
//! file. A fixture that cannot round-trip should not be forced through that
//! check: assert that it parses, or that it returns a specific error, and
//! say why in the test.

use std::fs;
use std::path::PathBuf;

use omatrack::Module;

struct Fixture {
    name: &'static str,
    bytes: &'static [u8],
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "10kdub_JAM_PD.mod",
        bytes: include_bytes!("data/10kdub_JAM_PD.mod"),
    },
    Fixture {
        name: "11thhour_TDK_CCBY.mod",
        bytes: include_bytes!("data/11thhour_TDK_CCBY.mod"),
    },
    Fixture {
        name: "dog2_songerson_CCBY.mod",
        bytes: include_bytes!("data/dog2_songerson_CCBY.mod"),
    },
    Fixture {
        name: "fx-poly1_k0wax_CC0.mod",
        bytes: include_bytes!("data/fx-poly1_k0wax_CC0.mod"),
    },
    Fixture {
        name: "k0w-rsnd_k0wax_CC0.mod",
        bytes: include_bytes!("data/k0w-rsnd_k0wax_CC0.mod"),
    },
    Fixture {
        name: "momentary_meditation_kjose_CCBY.mod",
        bytes: include_bytes!("data/momentary_meditation_kjose_CCBY.mod"),
    },
    Fixture {
        name: "mule10k_newtined_CCBY.mod",
        bytes: include_bytes!("data/mule10k_newtined_CCBY.mod"),
    },
    Fixture {
        name: "openmpt_test_BSD3.mod",
        bytes: include_bytes!("data/openmpt_test_BSD3.mod"),
    },
    Fixture {
        name: "particle_man_potajoe_CCBY.mod",
        bytes: include_bytes!("data/particle_man_potajoe_CCBY.mod"),
    },
    Fixture {
        name: "tiny_chip_m4kachu_CCBY.mod",
        bytes: include_bytes!("data/tiny_chip_m4kachu_CCBY.mod"),
    },
];

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn assert_same_bytes(name: &str, left: &[u8], right: &[u8]) {
    if left == right {
        return;
    }
    let shared = left.len().min(right.len());
    for index in 0..shared {
        if left[index] != right[index] {
            panic!(
                "{name}: byte {index}: {:02X} != {:02X} (lengths {} and {})",
                left[index],
                right[index],
                left.len(),
                right.len()
            );
        }
    }
    panic!("{name}: lengths differ: {} vs {}", left.len(), right.len());
}

#[test]
fn data_dir_lists_the_fixtures_and_their_licenses() {
    let dir = data_dir();
    let mut found = Vec::new();
    for entry in fs::read_dir(&dir).expect("tests/data") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|ext| ext == "mod") {
            found.push(
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    found.sort();
    let mut expected: Vec<&str> = FIXTURES.iter().map(|fixture| fixture.name).collect();
    expected.sort();
    assert_eq!(found, expected);

    let notes = include_str!("data/ATTRIBUTION.txt");
    for fixture in FIXTURES {
        assert!(
            notes.contains(fixture.name),
            "ATTRIBUTION.txt does not mention {}",
            fixture.name
        );
        let on_disk = fs::read(dir.join(fixture.name)).expect(fixture.name);
        assert_same_bytes(fixture.name, &on_disk, fixture.bytes);
    }
    assert!(notes.contains("CC0 1.0"), "{notes}");
    assert!(notes.contains("Public Domain"), "{notes}");
    assert!(notes.contains("CC BY 4.0"), "{notes}");
    assert!(notes.contains("BSD-3-Clause"), "{notes}");

    let license = include_str!("data/OPENMPT_LICENSE_BSD3.txt");
    assert!(license.contains("Redistribution and use in source and binary forms"));
    assert!(license.contains("OpenMPT Project Developers and Contributors"));
}

#[test]
fn freely_licensed_fixtures_round_trip() {
    assert_eq!(FIXTURES.len(), 10);
    for fixture in FIXTURES {
        let module = Module::from_bytes(fixture.bytes).unwrap_or_else(|err| {
            panic!("{} failed to parse: {err}", fixture.name);
        });
        assert_eq!(module.tag.as_str(), "M.K.", "{}", fixture.name);
        assert!(!module.display_title().is_empty(), "{}", fixture.name);
        let written = module.to_bytes().unwrap_or_else(|err| {
            panic!("{} failed to write: {err}", fixture.name);
        });
        assert_same_bytes(fixture.name, &written, fixture.bytes);
    }
}
